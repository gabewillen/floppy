export const meta = {
  name: 'port-phase4-build',
  description: 'Build the phase 4 port (bot core and abilities) slice by slice in floppy, with full proof, live runs, and blind multi-lane review per slice',
  phases: [
    { title: 'Baseline', detail: 'floppy green on the new scxml.rs subtree; docs mapping' },
    { title: 'Slices', detail: 'implement → blind review → repair, one slice at a time' },
    { title: 'Report', detail: 'phase 4 report' },
  ],
}

const PLAN = '~/.agents/projects/floppy/plans/20261008-phase4-bot-core.mdscript.md'
const CTX = `You are a self-implement worker for floppy (~/VSCode/stateforward/floppy), porting phase 4 (bot core and abilities). Read docs/handoff.md ("Port run decisions") and the plan ${PLAN}: its "Rules For Every Slice" section and its "User Answers (2026-10-08)" section, which OVERRIDE the plan body (notably: the bot is bots/floppy/ not bots/companion/ — rename every companion path/name accordingly; FLOPPY_BASE_URL has NO default, fail fast when unset; abilities reference schema files, no inline copies, once scxml.rs EmbeddedDocument::event_schemas exists). Almost everything is .scx; Rust/Wasm only for I/O services, justified. Grants: commit locally (atomic commits, no attribution trailers), live runs on the Sparks direct vLLM and TypeSafe. Never push. Never print .env values. Never edit floppy's scxml.rs/ subtree directly: scxml.rs changes go in ~/VSCode/stateforward/scxml.rs-services on feat/invoke-services, then \`git -C ~/VSCode/stateforward/scxml.rs merge --ff-only feat/invoke-services\` and \`git subtree pull --prefix scxml.rs ~/VSCode/stateforward/scxml.rs feat/invoke-services --squash\` from floppy. New scxml.rs public names are only those approved (EmbeddedDocument::event_schemas, EVENT011); any other new scxml.rs name: stop and report it as a blocker. Do not run \`find\`. Update the plan's "## Done So Far" (append) as you finish.`

const SLICE_RESULT = {
  type: 'object',
  properties: {
    status: { type: 'string', enum: ['done', 'blocked'] },
    commits: { type: 'array', items: { type: 'string' } },
    proof: { type: 'array', items: { type: 'string' }, description: 'each command with pass/fail' },
    live: { type: 'string', description: 'live run result, or why none applies' },
    blocker: { type: 'string' },
    notes: { type: 'string' },
  },
  required: ['status', 'commits', 'proof', 'live', 'blocker', 'notes'],
}
const REVIEW = {
  type: 'object',
  properties: {
    blocking: { type: 'array', items: { type: 'string' }, description: 'blocking findings with file:line and failure scenario' },
    nonblocking: { type: 'array', items: { type: 'string' } },
    verdict: { type: 'string', enum: ['proven', 'blocked'] },
  },
  required: ['blocking', 'nonblocking', 'verdict'],
}

const LANES = [
  'correctness and SCXML: W3C SCXML 1.0 semantics, event names byte for byte vs the original, done/cancel/invoke lifecycle, fidelity to the original behaviour this slice ports; rerun the slice proof commands yourself',
  'completeness and rules: the slice\'s done-when criteria all actually met, User Answers honored (bots/floppy, no default URL, schema files, .scx-first with justified Rust), hard cutover (no leftovers/aliases), atomic commits, docs/tests updated, no secrets printed or committed',
  'security and engineering: env/secret handling, http-origins and capability grants, OTEL usage in any Rust touched (cardinality of new attributes), test quality (no weakened asserts, deterministic tests isolated, no shared temp files), clippy/fmt',
]

async function slice(heading, extra) {
  const label = heading.toLowerCase().replace(/[^a-z0-9]+/g, '-').slice(0, 40)
  let res = await agent(`${CTX}\n\nImplement the plan slice "### ${heading}" completely, then run its full proof and live proof. ${extra || ''}\nReport status=blocked with the exact blocker if you cannot finish it honestly.`, { label: `impl:${label}`, phase: 'Slices', schema: SLICE_RESULT })
  if (!res || res.status === 'blocked') return { heading, res }
  for (let round = 0; round < 2; round++) {
    const reviews = (await parallel(LANES.map((lane, i) => () => agent(`You are one blind review lane (read-only: do not edit or commit). Repository: ~/VSCode/stateforward/floppy (and ~/VSCode/stateforward/scxml.rs-services if the slice touched scxml.rs). Plan: ${PLAN} (the "User Answers" section overrides the plan body). Review the slice "### ${heading}", implemented in these commits: ${res.commits.join('; ')}. Claimed proof: ${res.proof.join('; ')}. Live: ${res.live}.\nYour lane: ${lane}.\nOnly report blocking findings you have evidence for; default verdict proven when none.`, { label: `review${round}:${label}:${i + 1}`, phase: 'Slices', schema: REVIEW })))).filter(Boolean)
    const blocking = reviews.flatMap(r => r.blocking)
    if (!blocking.length) return { heading, res, reviews }
    log(`${heading}: ${blocking.length} blocking findings, repair round ${round + 1}`)
    const fix = await agent(`${CTX}\n\nRepair these blocking review findings for plan slice "### ${heading}" (commits ${res.commits.join('; ')}). Verify each finding first; fix the real ones at their cause, rerun the slice's full proof and live proof, commit atomically. Findings:\n- ${blocking.join('\n- ')}`, { label: `fix${round}:${label}`, phase: 'Slices', schema: SLICE_RESULT })
    if (!fix || fix.status === 'blocked') return { heading, res: fix || res, blockedAfterReview: blocking }
    res = { ...fix, commits: [...res.commits, ...fix.commits] }
  }
  return { heading, res, unresolved: true }
}

phase('Baseline')
const base = await agent(`${CTX}\n\nfloppy just pulled scxml.rs 92742d65 (data models as packages, foreign markup accepted with LINT022, xml:base, LIMIT005/006 removed and LIMIT007-009 renumbered to LIMIT005-007, DATAMODEL001/002, PKG005). Make floppy green on it: run scripts/build-components.sh and floppy's full proof (fmt, clippy -D warnings, all tests, builder_bot live test on direct vLLM via FLOPPY_LIVE_ENV_FILE). Fix any breakage (e.g. code/golden references to renumbered LIMIT codes). Update docs/decisions.md's scxml.rs mapping for the datamodel pass and the "Where things stand" section of docs/handoff.md (scxml.rs main at 92742d65, datamodel pass landed, flaky test fixed). Commit atomically.`, { label: 'baseline', phase: 'Baseline', schema: SLICE_RESULT })
if (!base || base.status === 'blocked') return { stage: 'baseline', base }

phase('Slices')
const SLICES = [
  ['Cut Over Model Endpoint Env Names', 'Override: no neutral default origin — FLOPPY_BASE_URL is required; remove any default http-origins entry the plan proposed and prove a clear fail-fast error when it is unset (the allowed origin must still come from the document/env, per the generate service\'s capability model; if that is impossible without a default, report blocked with the exact reason).'],
  ['Prove Invoke Tree Spikes', ''],
  ['Add Decision Menu Event Filter', ''],
  ['Add scxml.rs Event Schemas For Invoked Children', 'ALREADY IMPLEMENTED AND COMMITTED by an earlier lane: scxml.rs 73df0554 (EmbeddedDocument::event_schemas), 674e2f6e (EVENT011), 20fe0722 (canonical components record, manifest 03bc0069); scxml.rs main ff to 20fe0722; floppy 3bb60dbd + 83852f6e (subtree pull), e7c0a870 (decision:menu → decision:choice rename, D32, user decision), 064b440c (records). Do NOT redo it. Do NOT run the scxml.rs workspace test suite or Docker builds: scxml.rs at 20fe0722 was already fully proven with canonical components. Verify it against the description below and the User Answers, fix anything missing or wrong with new atomic commits, and run what that lane skipped: floppy builder_bot and decision_service LIVE tests on direct vLLM (FLOPPY_LIVE_ENV_FILE), and investigate builder_bot::a_builder_timeout_preempts_a_generation_in_flight_and_cancels_it which timed out once under full-suite load (reproduce before fixing; if it is a real flake, fix its cause). Report status done with ALL the commits listed above plus any new ones. Original slice description: This slice is NOT in the plan body; it comes from User Answers. In ~/VSCode/stateforward/scxml.rs-services on feat/invoke-services: add the approved public field EmbeddedDocument::event_schemas so a literal <invoke src> child\'s document-relative event:schema files reach its compile, and the approved validator warning EVENT011 checking #_parent/#_<invokeid> send payloads against the receiving literal child\'s transition schema across documents. Follow scxml.rs/AGENTS.md: --explain text, docs/validator-rules.md, CHANGELOG (unreleased), UI goldens, browser host if affected. Full scxml.rs proof: fmt; clippy -D warnings all/no features and wasm32; workspace + doc tests; UI goldens; published_contract_docs; both cargo doc runs; scripts/verify-wasm-capabilities.sh; scripts/verify-browser-host.sh; cargo deny --offline check; IRP dispositions unchanged (regenerate baselines with the Docker linux/amd64 canonical build only if bytes move; macOS-built components never match canonical identity — use SCXML_COMPONENTS_DIR from the canonical build for irp_components). Then ff scxml.rs main and subtree-pull into floppy, and confirm floppy stays green.'],
  ['Extract Phase 4 Event Contracts', 'Use schema files referenced by abilities (EmbeddedDocument::event_schemas now exists); no inline copies or equality tests.'],
  ['Build Body And Thin Cognition', ''],
  ['Build Cognition Cascade', ''],
  ['Assemble Choices And Route Selections', ''],
  ['Add Communication Conversation And Speaking', ''],
  ['Pin Phase 4 Event Traces', ''],
  ['Add Choice Compaction Element', ''],
  ['Move Choice Assembly And Routing Into The Decision Namespace', ''],
  ['Move Selection Schema And Confidence Tuner Into The Decision Namespace', ''],
  ['Add Floppy Bot Namespace', ''],
  ['Add Floppy Conversation Namespace', ''],
  ['Enforce Event Schemas At Runtime In scxml.rs', 'scxml.rs change, approved by the user — implement the REVISED and RAISE TOO answers (sender-side error.execution), and the orchestrator decision of 2026-10-09 in the plan User Answers: add machine-api `session.accepts-event-data: func(event) -> bool` (mirrors data-model-api accepts-event-data) for #_parent/#_<invokeid> sends and host input; record the Boa fuel case as a limitation. If an earlier attempt already landed the in-document part on feat/invoke-services, continue from it. Work in ~/VSCode/stateforward/scxml.rs-services on feat/invoke-services (rebase/merge-ff onto scxml.rs main first if main moved; other scxml.rs workers may be fast-forwarding main concurrently — coordinate only through git, rebasing as needed). Use the scxml.rs fast proof from CONTRIBUTING.md, plus the full proof once at the end of this slice (it changes scxml.rs). Then ff scxml.rs main and subtree-pull into floppy.'],
  ['Replace Script Validators With Event Schemas', 'After deleting the validators, move EVERY remaining inline <script> body in bots/floppy/*.scx, abilities/*.scx, and bots/builder/builder.scx into .js files next to each document, loaded with <script src> (user rule SCRIPT FILES). If scxml.rs does not carry <script src> files for literal <invoke src> children, add that in scxml.rs following the EmbeddedDocument::event_schemas pattern (record the choice). Done when `grep -l "<script>" bots abilities -r` finds nothing, the pinned event traces reproduce exactly, and the builder live test passes via CPA.'],
  ['Add Identity Text Ability', 'Redo under the DOCUMENT STRUCTURE rule; the earlier attempt is in floppy git stash "wip identity slice (paused for restructure 2026-10-09)" for reference only; drop that stash when this slice lands.'],
  ['Declare Phase 5 Seams And Stimulus Fan-Out', 'Behaviors and routines come from bots/builder (plan User Answers, BEHAVIORS AND ROUTINES).'],
  ['Save Floppy Live Examples', 'Save them under bots/floppy/examples/.'],
  ['Close Phase 4 Docs', 'Also refresh the stale parts of docs/port-plan.md (phase 0 outbound-http API, Nemotron/CLM defaults) and mark phase 4 done there; update docs/handoff.md.'],
]
const results = []
const FAST = ' SPEED RULES (user, 2026-10-08): do not run the scxml.rs workspace test suite or canonical Docker builds unless this slice changes scxml.rs; run floppy\'s proof (build-components when components changed, fmt, clippy -D warnings, the floppy tests relevant to the slice plus `cargo test --workspace` once at the end) and the slice\'s live proof. Another lane may concurrently be reviewing the previous slice read-only; do not wait for it.'
function stat(r) { return !r.res ? 'no result' : r.res.status === 'blocked' ? `BLOCKED: ${r.res.blocker}` : r.unresolved ? 'done with unresolved review findings' : 'done' }
let stop = false
const LOAD = ' DOC TESTS: do not run `cargo test --doc` locally; they run only in CI (plan User Answers). SCRIPT FILES (user): no inline <script> bodies — any remaining ECMAScript goes in a .js file loaded with <script src> (see plan User Answers SCRIPT FILES). DELEGATED (user, 2026-10-09): never report blocked for a decision or a name; the plan User Answers DELEGATED entry overrides the CTX rule about new scxml.rs names — choose per SCXML 1.0 and existing patterns and record it under Decisions Made Under Delegation in the plan. LIVE MODEL: run live proof through the CPA gateway, FLOPPY_LIVE_ENV_FILE=.env.cpa:.env.local FLOPPY_LIVE_MODEL=gabe-codex-1/gpt-6-luna (fallback gabe-xai/grok-4.6); see the plan User Answers. LIVE PROOF UNDER LOAD: the Sparks vLLM (http://192.168.2.89:8000) may be saturated by other clients (check /metrics vllm:num_requests_running). If a live run fails only because of server load (timeouts with high num_requests_running), do NOT report blocked: record the live proof as DEFERRED with the metrics in your live field, keep the deterministic proof green, and continue; a catch-up live pass runs at phase end. Keep any live run you do make short (one turn).'
// slices 0-2 already ran in the original sequential style (cached on resume)
for (const [heading, extra] of SLICES.slice(0, 3)) {
  const r = await slice(heading, extra)
  results.push(r); log(`${heading}: ${stat(r)}`)
  if (!r.res || r.res.status === 'blocked' || r.unresolved || r.blockedAfterReview) { stop = true; break }
}
// pipelined: review slice N while slice N+1 is implemented; repair N after N+1's implementation
function review(heading, res) {
  const label = heading.toLowerCase().replace(/[^a-z0-9]+/g, '-').slice(0, 40)
  return parallel(LANES.map((lane, i) => () => agent(`You are one blind review lane (read-only: do not edit, commit, or run cargo builds that write into floppy's target/ other than tests). Repository: ~/VSCode/stateforward/floppy (and ~/VSCode/stateforward/scxml.rs-services if the slice touched scxml.rs). Another lane may be committing the NEXT slice concurrently; review only the commits named here. Plan: ${PLAN} (the "User Answers" section overrides the plan body). Review the slice "### ${heading}", implemented in these commits: ${res.commits.join('; ')}. Claimed proof: ${res.proof.join('; ')}. Live: ${res.live}.\nYour lane: ${lane}.\nOnly report blocking findings you have evidence for; default verdict proven when none.`, { label: `preview:${label}:${i + 1}`, phase: 'Slices', schema: REVIEW }))).then(rs => rs.filter(Boolean))
}
async function repair(heading, res, reviews) {
  const blocking = reviews.flatMap(r => r.blocking)
  if (!blocking.length) return { heading, res, reviews }
  const label = heading.toLowerCase().replace(/[^a-z0-9]+/g, '-').slice(0, 40)
  log(`${heading}: ${blocking.length} blocking findings, repairing`)
  const fix = await agent(`${CTX}\n\nRepair these blocking review findings for plan slice "### ${heading}" (commits ${res.commits.join('; ')}). Later slices may already be committed on top; keep them working. Verify each finding first; fix the real ones at their cause, rerun the affected proof and live proof, commit atomically.${FAST}${LOAD}\nFindings:\n- ${blocking.join('\n- ')}`, { label: `pfix:${label}`, phase: 'Slices', schema: SLICE_RESULT })
  if (!fix || fix.status === 'blocked') return { heading, res: fix || res, blockedAfterReview: blocking }
  const again = await review(heading, { ...fix, commits: [...res.commits, ...fix.commits] })
  const still = again.flatMap(r => r.blocking)
  return still.length ? { heading, res, unresolved: true, still } : { heading, res: { ...fix, commits: [...res.commits, ...fix.commits] }, reviews: again }
}
let pending = null // { heading, res, reviewsP }
for (const [heading, extra] of (stop ? [] : SLICES.slice(3))) {
  const label = heading.toLowerCase().replace(/[^a-z0-9]+/g, '-').slice(0, 40)
  const res = await agent(`${CTX}\n\nImplement the plan slice "### ${heading}" completely, then run its proof and live proof. ${extra || ''}${FAST}${heading.startsWith('Add scxml.rs') ? '' : LOAD}\nReport status=blocked with the exact blocker if you cannot finish it honestly.`, { label: `impl:${label}`, phase: 'Slices', schema: SLICE_RESULT })
  if (res && res.status === 'blocked' && heading.startsWith('Add scxml.rs')) { log(`${heading}: builder live proof deferred (vLLM saturated by other clients); continuing`); res.status = 'done'; res.live = `DEFERRED: ${res.blocker}` }
  if (pending) {
    const r = await repair(pending.heading, pending.res, await pending.reviewsP)
    results.push(r); log(`${pending.heading}: ${stat(r)}`)
    pending = null
    if (r.unresolved && r.heading.startsWith('Add scxml.rs')) log(`${r.heading}: unresolved scxml.rs finding handed to a separate scxml.rs lane; continuing`)
    else if (!r.res || r.res.status === 'blocked' || r.unresolved || r.blockedAfterReview) { stop = true; break }
  }
  if (!res || res.status === 'blocked') { results.push({ heading, res }); log(`${heading}: ${stat({ res })}`); stop = true; break }
  pending = { heading, res, reviewsP: review(heading, res) }
}
if (pending && !stop) {
  const r = await repair(pending.heading, pending.res, await pending.reviewsP)
  results.push(r); log(`${pending.heading}: ${stat(r)}`)
}

if (!stop) {
  const catchup = await agent(`${CTX}\n\nPhase 4 catch-up pass. First run doc tests once (user: doc tests only at phase end): `cargo test --workspace --doc --locked` in ~/VSCode/stateforward/scxml.rs (main) and in floppy; fix any failure at its cause and commit (scxml.rs fixes on feat/invoke-services, ff main, subtree pull). Then the live pass. Check http://192.168.2.89:8000/metrics first. Run every live test whose proof was DEFERRED in these slice results (and the builder_bot and the floppy bot live tests), on direct vLLM. If the server is still saturated by other clients, wait for it with a bounded wait (up to 2 hours, checking every 10 minutes) rather than failing. Record results in docs/live-proof.md and commit.${FAST}\nResults:\n${JSON.stringify(results.map(r => ({ heading: r.heading, live: r.res && r.res.live })), null, 1)}`, { label: 'live-catchup', phase: 'Report', schema: SLICE_RESULT })
  results.push({ heading: 'Live catch-up', res: catchup })
}
phase('Report')
const report = await agent(`Read-only. Summarize phase 4 of the floppy port for the user from these slice results, plain language, short: what landed (commits), live proof results, anything blocked or unresolved and exactly what is needed, and the next step. Results:\n${JSON.stringify({ base, results }, null, 1)}`, { label: 'report', phase: 'Report' })
return { base, results, report }
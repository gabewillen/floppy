# Mapping floppy onto the original's domains

A proposal for the maintainer to review, written 2026-10-17. Q1 to Q7 have
been answered, and the answers are recorded where they apply. The first
step of Q10's order, the `processing` namespace with section 3's sends, is
implemented (2026-10-18, D113): each row it covers is marked **Done**, and
what it decided that the plan did not is under
[To review (step 1)](#to-review-step-1). The second, the `intuition` and
`cognition` namespaces, is implemented too (2026-10-18, D114), its rows
marked **Done** and its choices under [To review (step 2)](#to-review-step-2).
The third, the `identity`, `conversation` and `memory` namespaces, is
implemented (2026-10-18, D115), its rows marked **Done** and its choices
under [To review (step 3)](#to-review-step-3). It maps every custom element in `namespaces/*` and every
non-trivial function in `abilities/*/*.js` and `bots/*/*.js` onto the
original's modules, and proposes where each should live.

## The rules this proposal applies

The maintainer's words:

- "i wish we would have kept the domains as close to [the original] as possible where terminology didn't exist in scxml like dispatch is obviously send"
- "I wanted decision to be of the api like decision:choice"
- "I don't like tuner, route, verdict"
- "If we need JavaScript we should seriously consider a service or executable content element and use rust"
- "agree use scxml elements when possible" (answering Q2): wherever SCXML's own elements can do the job, use them, before a Rust element or a service. Choice (a) comes first, and (b) or (c) only where SCXML cannot do the job.

Together with the standing rule against invented names (when SCXML has the
concept, use SCXML's term; otherwise use the original's module and name),
they give four choices for each row:

| Choice | Meaning |
| --- | --- |
| (a) plain SCXML | Use SCXML constructs: `<send>`, `<foreach>`, `<param>`, a transition, `<cancel>`, an `event:schema`. |
| (b) Rust element | Executable content in a namespace named after the original's module (`processing`, `cognition`, `intuition`, ...). The element is named after the original's function or class. |
| (c) Rust service | An invoked service, for I/O or for state that must outlive a microstep. |
| (d) keep as is | Leave it where it is. |

How element names are formed (Q1, answered by the maintainer): the
original's name, except that SCXML's term replaces any word SCXML already
has ("dispatch is obviously send"), with no suffixes ("it's sending an
event we don't need a suffix"). So `dispatch_tool`, which writes the schema
of the events that may be sent, becomes `<processing:send location
choices>`. The tool's name on the wire stays `dispatch`, as in the
original's request. Words like `tool`, `args` and `events_from` are dropped
wherever SCXML's term covers them, and modal prefixes like `should_` are
dropped too (Q6: `should_escalate` becomes `<intuition:escalate>`). The
names this rule leaves unclear are listed under [Names to confirm](#names-to-confirm).

### Names to confirm

| Proposed | The original | Why it is unclear |
| --- | --- | --- |
| `<processing:model_facing_schema>` (E6), **taken overnight 2026-10-18 and implemented (D113); to review** | `model_facing_event_json_schema` | "event JSON schema" is what floppy's `event:schema` already says, so it shortens to `schema`. Whether `model_facing` stays is open; without it the name is `<processing:schema>`. |
| `<conversation:message_for_turn>` (E13), **taken overnight 2026-10-18 and implemented (D115); to review** | `_message_for_turn` | The rule renames today's `<conversation:commit>`, but "commit" is the name already in use and SCXML has no term for it. |
| E7's pin | none | The original has no name for it (Q11). |

Names kept unchanged under the rule, because SCXML has no term for them:
`collect_offered_events`, `selection_confidence`, `attention_bias_for_turn`, `model_facing_content`,
`observe`, `escalate` (from `should_escalate`, prefix dropped), `environment_actions`, `hears_name`,
`sorted_identities`, `identities_json`, `relationship_for_input`,
`complete_turn`, `content_for_detector`, `to_json`, `next_after`,
`lifetime_errors`, `verify_tick`, `replace_behavior_clauses`,
`grounded_triggers`, `behavior_input_payload`, `model_snapshot`.

## SCXML and repository constraints that shape the proposals

| Constraint | Source | What it means here |
| --- | --- | --- |
| Executable content cannot send | the extension world (`data-model-host`, `session-snapshot-host`; no send host); every namespace README says "it never sends" | Every dispatch has to be a `<send>`. An element can only work out *what* to send. |
| A transition is never chosen by a cond on who sent an event or what it is called | D81, AGENTS.md "Bots" | Routing has to come from event names and states. The kept exceptions are a re-entered state's `idlocation` id and the builder's untrusted child (D81, D82). A cond on a *selection's* data (`s.target`) is allowed, because that is data, not the event that is being processed. |
| Every non-trivial cond or expr calls a function in `<name>.js`, and each is at most 160 characters | D65, AGENTS.md | An expression is free only when it is a literal, an identifier or a dotted path (`s.event`, `s.target`, `s.data`). So `eventexpr="s.event"` and `<content expr="s.data"/>` need no JavaScript, D65 names only `cond` and `expr`, so an inline `targetexpr="'#_scxml_' + s.target"` is allowed as the rule is written (3.2). |
| Every document loads exactly one root `<script src="<name>.js"/>` | D65, AGENTS.md | A document left with no functions would still need a `.js` file. Q9 asks about relaxing this. |
| scxml.rs checks a send's data against the receiver's `event:schema` and raises `error.execution` in the sender | D41 | Data validation at dispatch comes for free with `<send>`. But it happens one send at a time: in a `<foreach>`, the sends before the failing one have already gone out. |
| An error inside `<foreach>` stops the loop | SCXML 4.6, 4.9 | A send that fails stops the selections after it, so dispatch could be partial. Q4 (answered) relies on schema validation before the send, so that this does not happen. |
| Services validate what they return; `done.invoke` donedata and service events are unchecked | AGENTS.md "Components", D41, D42 | Checking the shape of a generate result is allowed today, and is why the temporary JavaScript exists. A Rust element can do the same check with typed errors. |
| `scxml.rs/` is upstream-only | AGENTS.md | Nothing below requires an scxml.rs change. |

## 1. Element and function inventory

### 1a. Custom elements (15)

| # | floppy today (element, file, what it does) | The original's counterpart | Proposal | Reason |
| --- | --- | --- | --- | --- |
| E1 | `<bot:attention>` (`namespaces/bot`; `bots/floppy/floppy.scx`): writes the focus and the ordered focus candidates for the turn | `abilities/cognition/types.py` `attention_bias_for_turn` (with `bot.py` `_interrupt_reference`, `_target_device_reference`) | **Done (D114).** (b) `<cognition:attention_bias_for_turn>` | The original says cognition owns the attention policy, so it moves from `bot` to `cognition` under its own name. |
| E2 | `<bot:verdict>` (`namespaces/bot`; floppy, autonomy, candidates_runner, intuition, reasoning): removes `bot.ability.cognition.ignore` selections and returns the first message the attention policy refuses | `abilities/cognition/types.py` `without_ignore_selections`, `attention_selection_error`, both called from `types.dispatch_selected_events` | **Done (D113).** (a) dissolve it, with no fallback element (section 3.1; Q5 answered) | "verdict" is a floppy name. SCXML can express both checks with the constructs it already has. |
| E3 | `<bot:stimulus>` (`namespaces/bot`; 6 documents): builds the envelope `{event, data, source, target?, id?}` | `abilities/cognition/input.py` `InputData` stimulus (an `hsm.Event`'s fields) | (a) `<send>` with `<param name="event">`, `<param name="data">`, ... where the envelope is sent | Building an object from named values is exactly what `<param>` does. Caveat: bot-2 turns an empty source into `null`, and plain `<param>` does not. |
| E4 | `<bot:recognize>` (`namespaces/bot`; identity): says whether a text names the bot | `abilities/identity/recognition.py` `hears_name` | **Done (D115).** (b) `<identity:hears_name>` | Same logic, under the original's module and name. |
| E5 | `<decision:choice>` (`namespaces/decision`; 7 documents): writes the active state's `event:schema` transitions as choices | `abilities/processing.py` `enabled_call_events`; TypeSafe's `Choice` question (`criteria` keyed by label) | (d) keep in `decision` | This is the maintainer's example of the API (`decision:choice`): the choices are what goes into a TypeSafe `Choice` question. |
| E6 | `<decision:compact>` (`namespaces/decision`; cognition, communication, identity, reflection): strips `$schema`/`title`/`examples`; when over the offer bound it drops descriptions, and failing that writes `[]` | `abilities/processing.py` `model_facing_event_json_schema` (the bound is floppy's, D35) | **Done (D113).** (b) `<processing:model_facing_schema>` | It is the original's model-facing schema projection, which lives in `processing`. The bound stays an attribute. |
| E7 | `<decision:pin>` (`namespaces/decision`; floppy, reflection): sets a property's `enum` (the focus candidates for `bot.focus_device`) | No single function. The closest are `bot.py` `_ability_selected_configured_focus_device` (a guard) and the TypeSafe provider's `_candidate_values` (grounding from `enum`) | (b) move to `cognition`; name open (Q11). **Q11 taken: kept in `decision`, unrenamed (step 2).** | It belongs to the attention policy, not to the decision API. The original has no name for it to inherit. |
| E8 | `<decision:assemble>` (`namespaces/decision`; floppy): builds the turn's choices from the body's own plus each ability's offers, tagged by target, compacted, with size and over-bound | `abilities/processing.py` `collect_offered_events` | **Done (D113).** (b) `<processing:collect_offered_events>`, which tags every choice with its offering session's id, the body's own included (3.2) | Same job, same module. |
| E9 | `<decision:route>` (`namespaces/decision`; floppy, autonomy, candidates_runner, intuition, reasoning): puts each selection on the target that offers it; one that cannot be placed rejects them all | `abilities/processing.py` `dispatch_selected_events`, `_resolve_target`, `fill_unique_selection_targets` | **Done (D113).** (a) delete it: each stage sends with `<foreach>` + `<send>`, its target read straight from the selection's tag (section 3.2; Q3 answered) | "route" is a floppy name, and "dispatch is obviously send". The choices are already tagged by offerer, and the dispatch tool's `target` is a required enum of those tags, so nothing is left to place. |
| E10 | `<decision:names>` (`namespaces/decision`; 5 documents): writes the choices' names, for a log line | none | **Done (D113).** (a) delete it and use `<log expr="choices"/>` | It has no counterpart in the original and exists only for a log line. |
| E11 | `<decision:selection>` (`namespaces/decision`; reasoning, reflection): builds the dispatch tool's schema | `abilities/processing.py` `dispatch_tool`, `_selection_item_branch` | **Done (D113).** (b) `<processing:send location choices>` (the wire tool name stays `dispatch`) | It is the original's `dispatch` tool, so it gets the original's name (section 3.4). |
| E12 | `<decision:tuner>` (`namespaces/decision`; intuition): keeps `{mean, var, n}` and works out the escalation threshold | `abilities/cognition/intuition.py` `ConfidenceTuner` (`observe`, `threshold`, `should_escalate`) | **Done (D114).** (b) `<intuition:observe>` and `<intuition:escalate>`, with the state held in a `confidence_tuner` datum (Q6) | "tuner" is floppy's shortening. The original's method names avoid it, and the class lives in `intuition`. |
| E13 | `<conversation:commit>` (`namespaces/conversation`; conversation, speaking): builds one committed message and its relationship context reference | `abilities/communication/conversation/conversation.py` `_message_for_turn`, `_model_safe_content`, `_stable_context_ref`, `MessageProvenance` | **Done (D115).** (d) keep the `conversation` namespace; renamed `<conversation:message_for_turn>` (Q1, flagged below), its `keylocation` gone to `<conversation:relationship_for_input>` | The namespace already matches the original's module. |
| E14 | `<device:snapshot>` (`namespaces/device`; floppy): reads a device session's `{id, state, attributes, events}` | `device/device.py` and `environment/snapshot.py` (a device's state machine snapshot) | (d) keep | The domain already matches the original's `device` module. |
| E15 | `<migration:plan>` (`namespaces/migration`; migration): walks Alembic's revision graph | Alembic `ScriptDirectory.iterate_revisions` (the original uses Alembic under `abilities/memory/migrations`) | (d) keep; take in J41's version guard | The name is Alembic's own, so nothing is invented. |

### 1b. JavaScript logic (rows group functions that work as one unit)

| # | floppy today (functions, file, what they do) | The original's counterpart | Proposal | Reason |
| --- | --- | --- | --- | --- |
| J1 | `promptProjection`, `budgetMarkup`, `budgetCollection`, `budgetScalar`, `utf8Length`, `codePoints`, `xmlElement`, `xmlAttribute`, `pythonNumber`, `xmlField`, `xmlStimulus`, `xmlModels`, `xmlEpisode`, `xmlSelection`, `xmlBehavior`, `xmlDirective`, `xmlText` (`abilities/reasoning/reasoning.js`): render the reasoning prompt as the original's budgeted XML | `abilities/cognition/event.py` `model_facing_content`, `_project`, `_fill_field`, `_ProjectionBudget`, `_render_budgeted`; `processing.py` `InputData.model_facing_payload` | **Done (D114).** (b) `<cognition:model_facing_content>` | About 250 lines that parse, budget and serialize. This is exactly the code the "use Rust" rule is aimed at. |
| J2 | `xmlEnvironment`, `xmlAttributes`, `xmlTag` (`reasoning.js`): render the environment block | `environment/environment.py` `Environment.model_snapshot` | **Done (D114).** (b) part of J1's element, or a separate `<environment:model_snapshot>` | The original renders the block in `environment`. One element is simpler. |
| J3 | `dispatchFailure`, `generatedSelection`, `generatedFailure` (`reasoning.js` and `reflection.js`, both marked "Temporary"): read a generate result's `dispatch` tool calls into selections, or the original's error message | `providers/openai_compat/processing.py` `_events_from_dispatch_tool_calls`; `abilities/processing.py` `events_from_dispatch_args` | **Done (D113).** (a) delete it: nested `<foreach>` + `<send>` over the result's `tool_calls` and each call's `args.events`, with generate refusing a call to a function it did not offer (section 3.3; Q7 answered) | The `dispatch` tool's schema already makes every call's `events` valid, so nothing is left to read but the sends. |
| J4 | `recalledContents`, `parsedContent` (`reasoning.js`): parse memory rows as JSON objects and skip the rest | floppy cites `episodes_from_contents` and `directives_from_contents`; I could not find either in the original's tree. The original's memory has `OutputData.contents` (`abilities/memory/store.py`), whose readers (`names_from_output`) validate each content and skip the rest | **Done (D115).** (b) `<memory:contents location result>` (to review, step 3) | It parses and filters, but there is no confirmed name to use yet. |
| J5 | `assembledBehaviors` (`abilities/reflection/reflection.js`): turns the behavior rows and trigger rows into behavior objects | `abilities/cognition/inventory.py` `load_behavior` (rows in `behavior/storage.py`) | (b) `<behavior:...>`, named after `load_behavior` or the storage function | It merges two result sets and parses `examples`. |
| J6 | `distinctIntentTriggers`, `triggerInserts`, `replaceBehaviorStatements`, `replaceBehaviorWithRow`, `replaceBehaviorWithTriggers`, `behaviorRow*` (`reflection.js`): build a behavior's replace statements | `behavior/storage.py` `replace_behavior_clauses` | (b) `<behavior:replace_behavior_clauses>` | It builds the statements in order and removes duplicates. |
| J7 | `next_after` (`reflection.js`): the next occurrence of an `at` schedule under a POSIX TZ rule. It is pasted as JavaScript text into routine documents the builder writes | `behavior/schedule.py` `At.next_after` | (b) `<behavior:next_after>` | Generated documents could call the element instead of carrying 40 lines of pasted code. Builder drafts may use namespaces (D28). |
| J8 | `evidenceStimulusNames`, `evidenceTriggerNames`, `groundedTriggers`, `acceptedTriggers` (`reflection.js`) | `abilities/cognition/reflection/revision.py` `_evidence_trigger_names`, `_grounded_triggers` | (b) `<reflection:grounded_triggers>` | It filters and computes, and the original names it. |
| J9 | `turnBehaviorInput` (`reflection.js`), `behaviorInputPayload` (`abilities/autonomy/autonomy.js`): the behavior's input with focus added | `abilities/cognition/autonomy.py` `behavior_input_payload` | (b) `<autonomy:behavior_input_payload>`, or into `cognition` if one element is too small for its own namespace (Q10) | The same code is in two files and merges objects. |
| J10 | `writtenLifetime` (`reflection.js`), `draftLifetime` (`bots/builder/builder.js`): `persistent` when the document sends `bot.behavior.tick` | `behavior/instance.py` lifetime (the persistent routine contract) | (b) part of J44's `behavior` element | One rule, in two copies. |
| J11 | `renderedReport`, `abandonedMessage`, `builderFailedMessage` (`reflection.js`) | `behavior/diagnostic.py` `Report.render`; `revision.py` `Revision._abandon` | (d) keep | It only assembles message strings. |
| J12 | `canonicalJson`, `actionKey`, `turnOutput` (`abilities/cognition/cognition.js`): put environment actions first and drop a reasoning selection that repeats one (D87) | The original has no counterpart. It sends intuition's `_environment_actions` before reasoning runs, so nothing ever repeats | **Done (D113).** (a) intuition sends its environment actions when it escalates (`<foreach>` + `<send>`), and the dedupe is deleted | This restores the original's order and removes a floppy-only workaround. Decided by Q2 (answered). |
| J13 | `normalizedConfidence`, `selectionConfidence` (`abilities/intuition/intuition.js`) | `abilities/processing.py` `normalize_confidence`, `selection_confidence` | **Done (D113).** (b) `<processing:selection_confidence>` | Rounding half to even and taking the minimum is computation, and the original names it. |
| J14 | `isLowConfidence` (`intuition.js`) | `ConfidenceTuner.should_escalate` | **Done (D114).** (b) `<intuition:escalate>` (E12) | It goes together with E12. |
| J15 | `escalatedResult`'s filter (`intuition.js`): drops the deliberative handoff and the ignore | `abilities/cognition/intuition.py` `_environment_actions` | **Done (D113).** (a) send every selection: the handoff (`bot.ability.reasoning.input`) and the ignore go to their own offerers, which take them by transition. Fallback (b) `<intuition:environment_actions>` | Once dispatch is `<send>`, nothing needs filtering, because each event goes to the session that offers it. |
| J16 | `processorChoices`, `isDeliberativeHandoff` (`intuition.js`): remove the handoff from the decision choices | `providers/typesafe/processing.py` (skips `is_deliberative_handoff_schema`) | (c) in the `decision` service | The original does this in the TypeSafe provider, and the `decision` service is that provider. |
| J17 | `decisionState` (`intuition.js`): the TypeSafe state document `{"turn": {...}}` | `providers/typesafe/processing.py` `_stimulus_document` | (c) in the `decision` service, which takes the stimulus as a param | This is the provider's function too. |
| J18 | `pythonFloat`, `pythonVector`, `identityKey`, `codePointOrder`, `sortedIdentities`, `sameIdentity`, `identitiesJson` (`abilities/conversation/conversation.js`) | `abilities/identity/value.py` `sorted_identities`, `identities_json`, `normalize_identity` | **Done (D115).** (b) `<identity:sorted_identities>`; `identities_json` is part of `<memory:to_json>` (to review, step 3) | Sorting and serializing that must match Python byte for byte is fragile to write in JavaScript. |
| J19 | `unitVector`, `cosineSimilarity`, `trackRef` (`conversation.js`) | `identity/value.py` cosine similarity; `conversation.py` `_new_track_ref` | **Done (D115).** (b) the cosine is part of `<conversation:relationship_for_input>`; `trackRef` is `<conversation:new_track_ref>` | Vector arithmetic. |
| J20 | `relationshipIndexFor`, `relationshipScore`, `isSameParticipant`, `lowerScore`, `collectionScore`, `pairScore`, `bestAssignment` (`conversation.js`): choose the relationship an input belongs to by matching identity groups one to one | `conversation.py` `Conversation._relationship_for_input` (its identity collection score) | **Done (D115).** (b) `<conversation:relationship_for_input location indexlocation input>` | A one-to-one assignment search with similarity thresholds. |
| J21 | `relationshipsWithInput`, `relationshipsChanged`, `addedWithSource`, `relationshipsWithParticipant`, `relationshipsWithoutSource`, `relationshipsWithoutAdded`, `relationshipsWithMessage`, `detectorRefs` (`conversation.js`) | `conversation.py` `_participant_for_source`, `Conversation.detector_refs` | **Done (D115).** (b) `<conversation:participant_for_source>`, `<conversation:fail_turn>`, `<conversation:detector_refs>`; the new relationship is `<conversation:relationship_for_input>`'s; the failed remember's rollback and the sequence append are plain `<assign>`s (to review, step 3) | They filter and merge lists of relationships. |
| J22 | `memoryPayload`, `memoryContent`, `asciiJson`, `pythonJson` (`conversation.js`): the memory row's JSON exactly as Python writes it | `abilities/memory` `Memory.to_json` and `_encode_content` (as floppy cites them) | **Done (D115).** (b) `<memory:to_json location message>` | It reproduces Python's `json.dumps` byte for byte. |
| J23 | `turnProduct`, `turnProvenance` (`conversation.js`) | `conversation.py` `Conversation._complete_turn` | **Done (D115).** (b) `<conversation:complete_turn>`, which commits the message as E13 does | It joins texts with duplicates removed and chooses the content type. |
| J24 | `turnContent` (`conversation.js`) | `conversation.py` `_content_for_detector` | **Done (D115).** (b) `<conversation:content_for_detector>` | It decides what content the detector gets. |
| J25 | `isCommittedAppend` (`conversation.js`): matches an append's id, source and target against each committed provenance | `conversation.py` `_append_message` | (b) part of the `conversation` elements. **Kept in `conversation.js` (D115; to review, step 3)**: it searches the history, which never crosses the extension boundary | It searches the history. |
| J26 | `closedTurn`, `turnFailure`, `contentWith`, `contentTypeWith`, `textPartsWith`, `audioWith` (`abilities/turn_detector/turn_detector.js`) | `conversation/turn_detector/turn_detector.py` `TurnDetector._on_end`; `turn_detector/stimuli.py` | **Done (D115).** (b) `<conversation:ingest_content>` and `<conversation:on_end>`, after `_ingest_content` and `_on_end` (the name was open; to review, step 3) | It validates (refuses text and audio together) and builds the turn. |
| J27 | `speechProduct` (`abilities/interpretation/interpretation.js`) | `abilities/listening/interpretation.py` `_dispatch_product_cognition_input` | (a) `<send>` with `<param>`s | It just builds the event's data. `voice_embedding` is optional and gets its own `<if>`. |
| J28 | `isSegmentSourceMismatch`, `isSegmentRateMismatch`, `isSegmentFormatMismatch`, `segmentSourceMismatch`, `segmentConversionFailure` (`interpretation.js`) | `listening/interpretation.py` voice segment conversion | (b) `listening` namespace; name open | They check one field against another, which a schema cannot express. |
| J29 | `dropsSilence`, `handsOffSilence`, `classifiesSilence`, `identifiesDirectly`, `streamsFrame`, `streamsPcmFrame`, `isFirstLevelledFrame`, `isLouderFrame` (`interpretation.js`) | `listening/interpretation.py` stage selection by which children are configured | (d) keep | These are short conds on configuration and levels. They are D81-compatible because they test data, not the sender. |
| J30 | `observedGain`, `perceivedLevel`, `receivedLevel` (`abilities/sensitivity/sensitivity.js`) | `abilities/listening/sensitivity.py` `_score_against_prediction`, `_score_unproduced` | (d) keep, or (b) `listening` if J28 creates that namespace | Two subtractions are not worth a component on their own. |
| J31 | `turnOffers` (`bots/floppy/floppy.js`): adds the focused device's events to the offers | `bot.py` (the focused device's events offered to the model) | **Done (D113).** (b) part of `<processing:collect_offered_events>`, treating the device as one more offerer | In the original, a device is just another actor. |
| J32 | `environmentField` (`floppy.js`): the turn's environment from the bot's point of view | `environment/environment.py` `Environment.model_snapshot(perspective)` | (b) `<environment:model_snapshot>`, or J2's element | It is the same snapshot that J2 renders. |
| J33 | `routeChoices`, `isBodyRoute`, `isDeviceRoute`, `abilityTarget`, `deviceTarget`, `bodyRouteLog`, `abilityRouteLog` (`floppy.js`): choose `#_internal`, `#_<id>` or `#_scxml_<sessionid>` for each placed selection | `processing.py` `dispatch_selected_events`, the dispatch to each actor | **Done (D113).** (a) `<foreach array="selected" item="s"><send eventexpr="s.event" targetexpr="'#_scxml_' + s.target"><content expr="s.data"/></send></foreach>` in each stage, every choice tagged with its offering session's id (section 3.2) | There is no branching and no lookup. `abilityTarget`, `deviceTarget` and `routeChoices` go. |
| J34 | `focusDeviceEnabled`, `clearFocusEnabled`, `isFocusCandidate` (`floppy.js`) | `bot.py` `_ability_selected_configured_focus_device` (a guard); the clear-focus legality in `attention_selection_error` | (d) keep as transition conds | They are the original's guards, written as conds on data (allowed by D81). Section 3.1 makes them the attention check. |
| J35 | `isWaiting`, `waitingWithout`, `everyAbilityReady` (`floppy.js`) | none: readiness through `bot.ability.offers` (D35) | (d) keep | This is floppy's own bookkeeping. |
| J36 | `restCandidates`, `hasRest` (`abilities/candidates_runner/candidates_runner.js`), `restRoutines`, `hasRest` (`abilities/routines_runner/routines_runner.js`) | `autonomy.py` candidate loop; `routines.py` | (d) keep | The candidates run one at a time, waiting on each, so a `<foreach>` cannot do it. These are list slices. |
| J37 | `isRevisionChanged` (`abilities/routines/routines.js`): compares JSON | `abilities/cognition/routines.py` `_reconcile_activity` | (d) keep | A one-line compare. |
| J38 | `hasRespond` (`abilities/communication/communication.js`): the original's non-blank check `/\S/` (D42) | `communication.py` respond guard | (a) a `"pattern": "\\S"` on the text property in the event's schema | JSON Schema can express "not blank", so the check can leave the cond. |
| J39 | `selectedEvent`, `selectedData`, `hasEmptySelection`, `hasSingleSelection` (`reflection.js`) | `reflection.py` `_SelectedEvent`, `_matches_select_empty` | **Done (D113).** (a) the one selection is sent by 3.3's nested `<foreach>` to the reflection's own id; 3.3 covers the empty and the over-one cases | No element is needed. |
| J40 | `isGenerate` (`reflection.js`), `planGenerated`, `draftGenerated`, `isFrom`, `isOwnEvent`, `isOwnOrCandidateEvent` (`builder.js`): compare `_event.invokeid` with the fresh id | none (D81, D82 exceptions) | (d) keep | These are the sender checks that D81 and D82 keep because SCXML cannot express them by name. |
| J41 | `versionGuard`, `appliedStatements`, `canRetry` (`abilities/migration/migration.js`) | Alembic `command.upgrade`/`downgrade`, the version table update | (b) have `<migration:plan>` also write the guard and the ordered statements; `canRetry` stays | The plan already knows `from` and `to`. |
| J42 | `namedArgs` (`builder.js`): the arguments of the first call to a named function | the builder has no counterpart in the original (it is floppy's own bot) | (d) keep | A one-line filter. |
| J43 | `parseSchemaText`, `isSchemaValue`, `schemaErrors`, `parsedSchemas` (`builder.js`): the draft's `schemas[].schema` is JSON text that has to parse to a schema object | none | (a) make `schema` a JSON object in the author tool's parameters, so generate's schema check (D112) refuses a bad one | The service already validates, so the JavaScript parse is redundant. |
| J44 | `startTags`, `delayMs`, `delayedSends`, `scheduleEvents`, `lifetimeErrors`, `lifetimeHelp`, `seconds` (`builder.js`): find a draft's delayed sends with a regular expression and apply E0009 | `behavior/instance.py` `_lifetime_errors` | (b) `<behavior:lifetime_errors>`, which reads the draft with a real XML parser | Parsing SCXML with a regular expression is fragile, and the original has a name for this. |
| J45 | `applyErrors`, `nextSimResults`, `allTestsPassed`, `simulateDiagnostics` (`builder.js`): check the forced tick and the simulation results | `behavior/verify.py` `_verify_tick`, `_selection_binding_errors` | (b) `<behavior:verify_tick>` | It validates and compares. |
| J46 | `diagnosticsFingerprint`, `isNoProgress`, `validateDiagnostics`, `hasTestsWithinBounds`, `isReadyToSimulate`, `rejectedDraft` (`builder.js`) | `revision.py` `_abandon` ("same diagnostic message after fix") | (d) keep | Short sorts and maps over the validator report. |
| J47 | `isOptionalBound`, `requestBound`, `requestConstraints`, `requestMaxAttempts`, `requestModel`, `requestGenerateTimeoutMs`, `requestGenerateCeilingMs`, `isOptionalGenerateBound` (`builder.js`): check `builder.request`'s optional bounds | none | (a) put the bounds in `bots/builder/schemas/builder.request.json` | Payload checks belong in `event:schema` (D42). |

Skipped as simple glue: the result and log builders (`*Result`, `*Log`,
`*Message`, `*Failure`, `*Output`), delays (`*Delay`), statement builders that
make one row (`*SelectStatements`, `*InsertStatements`), and one-expression
conds (`isCurrentTurn`, `hasNoDatabase`, `isTimer`, ...).

## 2. Proposed namespace layout

| Namespace | The original's module | Holds | Comes from |
| --- | --- | --- | --- |
| `decision` | TypeSafe decision API (`system_one(state, questions)`; questions `Choice`, `Score`, `Noul`) | `<decision:choice>` (the `Choice` question's choices), and `<decision:pin>` until it is named (Q11). `score` or `noul` only when a document asks such a question (none does today). | keeps E5 and E7; loses `compact`, `assemble`, `route`, `names`, `selection`, `tuner` |
| `processing` | `abilities/processing.py` | `model_facing_schema`, `collect_offered_events`, `send`, `selection_confidence` | E6, E8, E11, J13, J31 |
| `cognition` | `abilities/cognition/types.py`, `event.py`, `input.py` | `attention_bias_for_turn`, `model_facing_content` (with the environment block); the focus pin stays in `decision` (Q11) | E1, J1, J2 (**Done, D114**) |
| `intuition` | `abilities/cognition/intuition.py` | `observe`, `escalate` (ConfidenceTuner's `observe` and `should_escalate`) | E12, J14 (**Done, D114**) |
| `identity` | `abilities/identity/recognition.py`, `value.py` | `hears_name`, `sorted_identities` (`identities_json` is part of `<memory:to_json>`) | E4, J18, J19 (**Done, D115**) |
| `conversation` | `abilities/communication/conversation/` | `message_for_turn` (was `commit`), `relationship_for_input`, `participant_for_source`, `fail_turn`, `detector_refs`, `new_track_ref`, `content_for_detector`, `complete_turn`, and the turn detector's `ingest_content` and `on_end` | E13, J19 to J26 (**Done, D115**; J25 kept) |
| `memory` | `abilities/memory/` | `to_json`, `contents` (the row parse, J4) | J4, J22 (**Done, D115**) |
| `behavior` | `behavior/` (`schedule.py`, `instance.py`, `verify.py`, `storage.py`) | `next_after`, `lifetime_errors`, `verify_tick`, `replace_behavior_clauses`, behavior row loading | J5 to J7, J10, J44, J45 |
| `reflection` | `abilities/cognition/reflection/revision.py` | `grounded_triggers` | J8 |
| `autonomy` | `abilities/cognition/autonomy.py` | `behavior_input_payload` (or fold it into `cognition`, Q10) | J9 |
| `listening` | `abilities/listening/` | segment conversion checks (optional) | J28 (J30 optional) |
| `environment` | `environment/environment.py` | `model_snapshot` (only if it is not folded into `cognition:model_facing_content`) | J2, J32 |
| `device` | `device/` | `snapshot` (kept) | E14 |
| `migration` | Alembic | `plan` (kept, takes in J41) | E15, J41 |
| `bot` | — | **disappears** | `attention` goes to cognition (**done, D114**), `verdict` is dissolved (**done, D113**), `stimulus` becomes `<param>`, `recognize` goes to identity (**done, D115**) |

Every new namespace is one more Wasm component, with its own crate, manifest,
XSD, IRP suite and entry in `scripts/build-components.sh`. Q10 asks whether
to create a namespace only when its first element lands.

## 3. The specific cases

### 3.1 `<bot:verdict>`: dissolved into SCXML

The original's `cognition.types.dispatch_selected_events` does two checks
before any dispatch:

| Check | The original | Proposed SCXML |
| --- | --- | --- |
| Remove the ignore | `without_ignore_selections`: the ignore "is not a body/device dispatch target (cognition only)" | Tag `bot.ability.cognition.ignore` with the sending stage's own session id, so the stage sends it to itself and takes it with a targetless transition that logs it. Sent like every other selection, it reaches the one session that takes it. Nothing is filtered and no cond is needed. |
| Focus legality | `attention_selection_error`: `focus_device` outside the candidates; `clear_focus` for another target or with no focus | `focus_device` is offered only while there are candidates (`focusDeviceEnabled`), and its `device` enum is pinned to them (E7), so scxml.rs's send check refuses any other device (D41). `clear_focus` is offered only by the bot and only while it holds a focus (`clearFocusEnabled`). The body's transition cond `isFocusCandidate` is the original's guard, `_ability_selected_configured_focus_device`. |

What changes from the original: a refused focus becomes generate's
`schema-mismatch`, raised when the pinned `device` enum fails, instead of the
original's `SelectionRejectionError` message. It is still caught before
anything is sent (Q4). Decided (Q5, "yes"): there is no fallback element,
and losing the original's focus wording is accepted.

### 3.2 `dispatch_selected_events` becomes `<send>`

The original: validate every selection (offered, data valid for the offered
event, target resolved, data valid for the target's declared event), then
dispatch them all, grouped by target. Reasoning, intuition and autonomy each
call it themselves (`reasoning.py:536`, `intuition.py:808`,
`autonomy.py:2189`). In floppy today, the stages check and the body sends.

Decided (Q2, Q3): each stage sends its own selections with plain SCXML, and
nothing places them first (the maintainer: "what is resolve_target we added
so much ceremony"). The choices are already tagged with the ability that
offers them, and the dispatch tool's `target` is a required enum of those
tags, so the selection already names its target. The form given:

```xml
<foreach array="selected" item="s">
  <send eventexpr="s.event" targetexpr="s.target ? '#_' + s.target : '#_internal'">
    <content expr="s.data"/>
  </send>
</foreach>
```

**That expression only works in the body, and only for abilities.** I checked
what each address reaches:

| Sender and tag | `'#_' + s.target` or `#_internal` reaches | Needed |
| --- | --- | --- |
| The body, an ability tag (`communication`) | `#_communication`, the body's own invoked child (invoke ids equal the ability keys) | correct |
| The body, an untagged body event (`bot.focus_device`, `bot.clear_focus`, decision-6 writes the document's own choices untagged) | `#_internal`, the body itself | correct |
| The body, a device tag (the device's reference, J31's `turnOffers`) | `#_<reference>`, but the bot does not invoke devices; the environment does (D105). The device lives at `#_scxml_` + `owned_devices[reference]` (`deviceTarget`, D110) | wrong: `error.communication` |
| A stage (reasoning inside cognition), an ability tag | `#_communication` names *the stage's own* child (SCXML 6.4: `#_<invokeid>` is a session this session invoked), not the body's ability | wrong |
| A stage, an untagged body event | `#_internal`, the stage itself, not the body | wrong |

Decided (the maintainer: "aren't targets just the id of the device?", then
"yes"): every choice is tagged with the session id of whatever offers it.

| Offerer | Tag (`target`) |
| --- | --- |
| A held device | its session id, `owned_devices[reference]` |
| An ability | `<body _sessionid>.<key>` (D110 fixes a child's session id) |
| The body's own choices | the body's `_sessionid` |

Every stage sends the same way, and so does the body, with no map and
nothing passed down, because `#_scxml_<sessionid>` reaches any session of
the run (D104):

```xml
<foreach array="selected" item="s">
  <send eventexpr="s.event" targetexpr="'#_scxml_' + s.target"><content expr="s.data"/></send>
</foreach>
```

**A deliberate difference from the original:** the model's `target` enum
shows session ids. The original shows actor keys: a device's reference, an
ability's snake_case class name, and `bot` for the body.

What this changes before the send:

| Where | Change |
| --- | --- |
| `<processing:collect_offered_events>` (E8, J31) | It writes each offer's tag as the offering session's id, not the ability's member name or the device's reference, and tags the body's own choices with the body's `_sessionid` instead of leaving them untagged (decision-6 today). The abilities' `bot.ability.offers` and the device snapshots it reads already give it those sessions. |
| `<processing:send>` (the `dispatch` tool's schema, E11) | Its `target` enum lists those ids. Nothing else changes. |
| The intuition's target, from the `decision` service | The service's single-offerer target (the original's `_single_enabler`) is the offering choice's tag, so it is a session id too. |
| Autonomy's behavior contract | A learned behavior names its selections' `target` as a session id. The contract (reflection's `turnContract`) has to give the behavior the ids it may name, for example the turn's choices with their tags. |

D65 and the inline `targetexpr`: D65 and AGENTS.md say "every `cond` and
every `expr` outside `<data>`" that is more than a literal, an identifier or
a dotted path calls a function of `<name>.js`, and `tests/tests/contracts.rs`
checks only those two attributes (their 160-character bound). Read to the
letter, `targetexpr` is neither, so `"'#_scxml_' + s.target"` inline is
allowed. Today's practice still calls a function (`targetexpr="deviceTarget(route.target)"`).
I agree inline is fine. I recommend that D65's text name `targetexpr` and
`eventexpr` explicitly as attributes that may hold a short inline
expression, so that the reading is written down rather than implied.

| Part of the original | Where it goes |
| --- | --- |
| `fill_unique_selection_targets`, `_resolve_target` | Dropped. A model selection's `target` is required, and is limited to the offering tags by the dispatch tool's schema, which generate checks (D112). The intuition's selections come from the `decision` service, which returns no target today ("the target is the document's to resolve"). The original's TypeSafe provider sets it with `_single_enabler` (the only offerer, else none), so this moves into the `decision` service, (c). |
| "not offered", a target that does not offer the event, and invalid data | Decided (Q4): schema validation does it before anything is sent. generate validates each tool call's args against the `dispatch` tool's parameters schema (its `event` and `target` enums and each event's data schema) before it returns, so an invalid selection fails the whole call (`schema-mismatch`) and nothing is sent. No checking element is needed. |
| `validate_event_data` against the declared event | scxml.rs checks it at send time against the receiver's `event:schema` (D41); the services have already validated it against the choices' schemas |
| The dispatch itself, grouped by target, all awaited | `<foreach>` + `<send>`. SCXML sends are fire-and-forget and in order, and the order between two sessions is kept, so the original's per-target concurrency is not needed |
| Who dispatches | Each stage that dispatches in the original (reasoning, intuition, autonomy). The body sends only the trusted seed's selection and `bot.processing.completed` |

Constraints: an element cannot send, so `<send>` is unavoidable, and it is
the SCXML term. A send that fails inside `<foreach>` would stop the loop after
earlier selections had gone out. Q4 relies on schema validation before the
send so that it does not happen. Autonomy's learned
behaviors are trusted and produce their own selections, which the
original's dispatch fills with the only offerer. Here a behavior has to name
its `target` itself, a requirement for the turn behavior contract
(reflection's `turnContract`).

### 3.3 Reading a generate result's `dispatch` calls

Today, `dispatchFailure`, `generatedSelection` and `generatedFailure` are
temporary JavaScript, copied into both `abilities/reasoning/reasoning.js` and
`abilities/reflection/reflection.js`.

Decided (Q7): no element. Q3 and Q4 leave nothing for one to do. generate
validates every call against the `dispatch` tool's schema before it returns,
and the stage sends with nested plain SCXML in its `generate.result`
transition:

```xml
<foreach array="_event.data.tool_calls" item="call">
  <foreach array="call.args.events" item="s">
    <send eventexpr="s.event" targetexpr="'#_scxml_' + s.target"><content expr="s.data"/></send>
  </foreach>
</foreach>
```

Each of the original's checks (`_events_from_dispatch_tool_calls`,
`events_from_dispatch_args`) and where it ends up:

| The original's check | Where it goes |
| --- | --- |
| "processing expected tool 'dispatch', got: ..." (a call to another function) | Into generate's offered-function check. **This changes `services/generate`**: today a call of another name "has no schema here and is returned as made" (`services/generate/src/lib.rs`), and it would instead fail the reply (`schema-mismatch`), so `tool_calls` only ever holds calls of the offered function |
| "dispatch tool args must include an events array" / "must be an array of event selections" | The `dispatch` schema: `events` is a required array (`required: ["events"]`) |
| "dispatch selected unavailable event: ..." | The schema's per-branch one-value `event` enum |
| Each selection's data against its event's schema | The schema's per-event data branch (D112, before anything is returned) |
| Merging every dispatch call's events, in order | The outer `<foreach>`: calls in order, events in order |
| `fill_unique_selection_targets` | Not needed: `target` is required and is the offerer's session id (3.2) |

What the stage still needs, checked against reasoning and reflection:

| Need | Plain SCXML answer |
| --- | --- |
| Whether anything was selected | Taken by event name: `generate.result` means a valid selection, possibly empty (`events: []` selects none, as in the original), and `generate.failed` means none. A cond is needed only to tell empty from non-empty (below). |
| The selections themselves, for the stage's donedata (`selected`, which the cognition turns into `bot.processing.completed`'s `output`, the reflection's `cognition_output` and the stored episode) | Inside the inner loop, `<assign location="selected[selected.length]" expr="s"/>` appends each selection with no `.js` function (`expr` is an identifier; D65 constrains `cond` and `expr`, not `location`). Check that scxml.rs's ECMAScript data model accepts that location. If it does not, a one-line `.js` function such as builder.js's `appended` does it. |
| The ignore | Its tag is the sending stage's own session id (3.1, Q5), so the stage sends it to itself and a transition takes it. A `#_scxml_` self-send goes on the external queue, so it arrives after the current macrostep. If the stage has reached its final state by then, the ignore is dropped, which is harmless because it only logs. |
| Reflection: at most one selection, and empty means no change | Reflection's choices are its own transitions, tagged with its own id, so the one selection goes to itself and its transition applies it (D60's `_SelectedEvent` route, unchanged in effect). For "at most one", the reflection's `dispatch` schema can carry `maxItems: 1` on `events`, so generate refuses two. That needs one attribute on `<processing:send>`, `max` (not in the original's `dispatch_tool`; Q7a answered A), and the reflection's own cond and message go. For empty, a trailing `<send>` of a marker to its own id after the loop (the pattern of the body's `floppy.routed` today) arrives after any selection, because the external queue is FIFO. The reflection takes it as "no change" only while it is still waiting. |

Constraint: `_event.data.tool_calls` comes from a service event, which
scxml.rs does not check (D41). generate's own validation is what guarantees
its shape, which is acceptable because services MUST validate what they
return (AGENTS.md, Components).

### 3.4 `<decision:selection>` is the original's `dispatch_tool`

It builds the original's `dispatch` function tool's parameters: one `anyOf`
branch per offered event, `event` pinned by a one-value enum, the event's
data schema, and `target` as an enum of the offering targets. It belongs in
`processing` (`abilities/processing.py` `dispatch_tool`,
`_selection_item_branch`) as `<processing:send location choices>`: by Q1's
rule, SCXML's `send` replaces "dispatch" and the `_tool` suffix is dropped.
The tool's name on the wire stays `dispatch`, as in the original's request.
It is not part of the TypeSafe API, so it leaves `decision`.

### 3.5 `<decision:route>` and `<decision:tuner>`

- `route`: replaced by 3.2's `<foreach>` + `<send>` in each stage. Nothing places the selections first, and the name "route" goes away.
- `tuner`: `<intuition:observe confidence="..." location="confidence_tuner"/>` moves the state, and `<intuition:escalate location="escalate" confidence="..." tuner="confidence_tuner"/>` writes the boolean that today's `isLowConfidence` works out. Both are `ConfidenceTuner`'s own method names, and the class's defaults stay Rust constants. The state stays in the data model and is passed by `<param>` across turns, as today (handoff, "tuner passed by `<param>`"). It is not a service, because it does no I/O and is updated within a microstep.

## 4. Open questions (ask one at a time, in this order)

| # | Question | Recommendation |
| --- | --- | --- |
| Q1 | **Answered.** Element names: the original's, verbatim or shortened? | The original's name, but SCXML's term wherever SCXML has one, and no suffixes ("dispatch is obviously send"; "it's sending an event we don't need a suffix"). So `dispatch_tool` becomes `<processing:send location choices>`, and the wire tool name stays `dispatch`. Applied throughout; the unclear results are under [Names to confirm](#names-to-confirm). |
| Q2 | **Answered.** Who sends a turn's selections: each stage that dispatches in the original, or the body as today? | Each stage (reasoning, intuition, autonomy) sends its own selections with plain `<foreach>` + `<send>`, as the original does. The body keeps only the trusted seed. D87's dedupe (J12) goes. The maintainer: "agree use scxml elements when possible", now the general rule (see the rules at the top). |
| Q3 | **Answered.** Is a placement element (`resolve_target`) needed? | No ("what is resolve_target we added so much ceremony"). Each stage sends with `<foreach>` + `<send>`, its target taken from the selection's tag. Section 3.2 shows that the given `'#_' + s.target` / `#_internal` form works only in the body and for abilities. Map or session-id tags: **answered**, session-id tags ("aren't targets just the id of the device?", "yes"). Each choice's `target` is its offerer's session id, and every stage sends to `'#_scxml_' + s.target`. The model sees ids, a deliberate difference from the original's actor keys. |
| Q4 | **Answered.** With no placement step, what stops an invalid selection being sent after earlier ones have gone out? | Schema validation ("ideally that doesn't happen with schema validation"). generate validates each tool call's args against the `dispatch` tool's parameters schema (the `event` and `target` enums and each event's data schema) before it returns, so an invalid selection fails the whole call (`schema-mismatch`) before anything is sent. The plain `<foreach>` + `<send>` only ever sees valid selections, and no checking element is needed. The same holds on the other paths: the `decision` service validates the chosen data against the choices' schemas and sets `target` with the original's `_single_enabler` (3.2), and autonomy's trusted behaviors name their `target` themselves through the behavior contract. |
| Q5 | **Answered.** Dissolve `<bot:verdict>` into SCXML (3.1)? | Yes, with no fallback element. The ignore is a send to the stage's own id, taken by a transition. A bad focus device fails as generate's `schema-mismatch` through the pinned enum. The original's focus wording is accepted as lost. |
| Q6 | **Answered.** Replace `<decision:tuner>`? | Yes, with `<intuition:observe>` and `<intuition:escalate>`. The maintainer offered "intuition:escalate or intuition:think"; `escalate` is the original's word, and team-lead is confirming it. The naming rule now drops modal prefixes like `should_`. |
| Q7 | **Answered.** Is an element needed to read the generate result's `dispatch` calls? | No. Nested `<foreach>` over `_event.data.tool_calls` and `call.args.events` with `<send>` (3.3). generate refuses a call to a function it did not offer (a `services/generate` change), and the schema covers the original's other checks. Reflection's "at most one" and "empty" are handled as 3.3 says. |
| Q7a | **Answered.** Reflection's at most one: `maxItems: 1` in the schema (A), or keep today's check (B)? | A ("A"). `<processing:send>` takes one attribute, `max`, which puts `maxItems` on the tool's `events` array. generate's schema check then refuses a second event as `schema-mismatch` before anything is sent, and reflection keeps no check of its own. |
| Q8 | **Answered.** `services/decision`'s kinds (`choose`, `select`, `rank`, `classify`) are floppy's. Whose terms should they follow? | OpenAI Decisions API's question types ("use the openai decision api", then "Agree" to names, not backend): `choose` and `classify` become `choice`, `select` becomes one `predicate` per choice, `rank` becomes `score`. TypeSafe's `/v1/systemone` stays the backend; a `/v1/decisions` backend is a later swap with no rename. A separate change after this plan. |
| Q9 | When a document has no functions left, may it drop its `<name>.js` and `<script src>`, which D65 and AGENTS.md currently require? | Yes. Amend D65 to "at most one root `<script src>`". **Taken on the recommendation overnight 2026-10-18 while the maintainer slept ("finish this up tonight to parity"); to review.** Not exercised by step 1: every document still has functions, so D65 is unchanged. |
| Q10 | Create every namespace in section 2 now, or each only when its first element lands (and fold one-element modules like `autonomy` into `cognition`)? | One at a time, in this order: `processing`, then `intuition` and `cognition`, then `identity`, `conversation` and `memory`, then `behavior`. `autonomy` stays its own namespace, following the original's module boundary (keep-the-original's-domains rule). **Taken on the recommendation overnight 2026-10-18 while the maintainer slept ("finish this up tonight to parity"); to review.** |
| Q11 | `<decision:pin>` has no counterpart name in the original. Keep it as a `cognition` element (which name?), or drop it and rely on the body's guard (`isFocusCandidate`), as the original does? | Keep the pin as an element, unrenamed (`<decision:pin>` stays where it is) until the maintainer names it: no name is invented. **Taken on the recommendation overnight 2026-10-18 while the maintainer slept ("finish this up tonight to parity"); to review.** |
| Q12 | Delete `<decision:names>` and log the choices with `<log expr="choices"/>`? | Yes: delete `<decision:names>`, use `<log expr="choices"/>`. **Taken on the recommendation overnight 2026-10-18 while the maintainer slept ("finish this up tonight to parity"); to review.** **Done (D113).** |

## To review (step 1)

Choices step 1 (2026-10-18, D113) made that the plan did not settle, each
the closest to the original found, each for the maintainer to review:

1. **The ignore goes to the cognition, not to the stage.** Section 3.1 tags `bot.ability.cognition.ignore` with the sending stage's own id. It is the cognition's offer, so `<processing:collect_offered_events>` tags it with the cognition's session id, and every stage sends it there; the cognition's existing transitions log it. That is the original's "cognition only" (`without_ignore_selections`) and needs no re-tagging in each stage.
2. **The body sends itself the selections that name no target.** Section 3.2 says the body sends only the trusted seed's selection; the seed's SpeechHeard selection names no target (the original resolves it among the bot's declared events), so on the turn's output the body sends `#_internal` each selection whose `target` is not a string (`hasNoTarget(s)` in `floppy.js`, a cond on data, D81). Every other path names a target.
3. **J31's device offer is two attributes, `devices` and `focus`.** `<processing:collect_offered_events>` reads the turn's device reads (`device_snapshots`) and the focused device's reference, and offers that device's events tagged with its snapshot `id`, sorted by its reference among the children's invoke ids (today's order). The original has no attribute names for this.
4. **Two marker events take the original's names.** Reflection's trailing self-send (3.3's empty case) is `bot.ability.reflection.selected`, the original's name for the selected-event completion; the candidates runner raises `bot.ability.autonomy.candidate.dispatch.succeeded` after its sends, and an `error.*` raised before it (a send that failed) fails the candidate (`Autonomy behavior dispatch failed: <error>`), keeping the old "a selection that cannot be placed fails the turn" test.
5. **A selection without `data` is sent with `{}`.** The dispatch tool requires `data` only when its schema requires a field (the original's), and the original's `dispatch_selected_events` dispatches a selection with no data (or a non-object one) with `{}`; every stage sends `<content expr="selectionData(s)"/>`, a one-line function of its `.js` that does the same. **Resolved 2026-10-18.**
6. **An event several sessions offer reaches none from intuition.** The decision service names a target only for a single offerer (`_single_enabler`); with none, `'#_scxml_' + undefined` fails the send inside intuition's last transition and is lost (the stage still answers handled). The original fails such a selection in `_resolve_target`.
7. **`<processing:selection_confidence>` reads a number in (0, 1] as a probability.** The original also scales only floats in (0, 1]; the data model cannot tell the integer 1 from 1.0, so 1 is read as certainty (100), as intuition's JavaScript did.
8. **The `choices` <param> of autonomy and the candidates runner is gone.** It only fed `<decision:route>`; the turn behavior contract now asks a behavior to name each selection's `target` as this turn's `cognition_output` does (session ids are stable: the root is always `session`).
9. **The body's `bot.focus_device` guard is the turn's candidates at any time.** It used to apply only while routing (`offering`/`routing_selections`); a stage's send arrives during `processing`, so the guard is `focus_candidates.length > 0` and `bot.clear_focus`'s is a focused device, with the `offering` flag gone.
10. **The turn's output is the handling stage's selections.** Intuition's environment actions are sent by intuition and no longer carried in the output (`bot.processing.completed`, reflection's `cognition_output`, the stored episode), as the original's cognition output is its terminal stage's.
11. **Logged choices are whole JSON.** `<log expr="choices"/>` (Q12) logs the choices with their schemas, not names; the tests read them with `support::choice_names`.
12. **Session-id targets degrade the local model (live, 2026-10-18).** With the dispatch tool's `target`s as session ids, `Qwen/Qwen3.8-Flash-Next` put `bot.focus_device`'s `device` beside `event` instead of in `data` in `live_choices`; the same request with the original's actor keys, or with no `target` on that branch, was filled correctly (docs/live-proof.md). Offering actor keys and sending to their sessions would reopen Q3.

## To review (step 2)

Choices step 2 (2026-10-18, D114) made that the plan did not settle, each
the closest to the original found, each for the maintainer to review:

1. **A budget's message goes to `failurelocation`.** The original's `model_facing_content` raises `ValueError("prompt projection ... budget exceeded")`, and reasoning fails the turn with that message. SCXML's `error.execution` carries no message, so `<cognition:model_facing_content>` writes it to an optional `failurelocation` (reasoning's `prompt_failure`) and `null` to `location`; without `failurelocation` a budget is `error.execution`. The attribute follows floppy's `<x>location` pattern (`focuslocation`, `overlocation`); the original has no name for it.
2. **The element's inputs are named after the `ProcessorInput`'s fields.** `stimulus` (rendered as `host_input`'s `processing:input`), `prior_episodes`, `standing_directives` and `environment`, plus `instructions` and `schema` for the prompt budget. The original's `InputData` takes `instructions` and `schemas` and builds the dispatch tool itself; here `schema` is the tool's parameters `<processing:send>` already built, which is what the JavaScript measured.
3. **Object members render in sorted order.** The data model hands an extension canonical JSON (sorted keys), where the JavaScript iterated insertion order. Everything that crossed a session or a `<param>` was sorted already, so the prompts are byte for byte the same (the element's unit tests compare eight inputs rendered by the JavaScript). Only memory rows parsed inside reasoning (`prior_episodes`, `standing_directives`) kept the original's Python insertion order before, and their nested objects now render sorted.
4. **Each input is bounded by the WIT value bound.** The element reads its inputs and writes the prompt through `data-model-host`, each value at most 65536 bytes (scxml.rs's ECMAScript data model). The prompt already crossed that bound as generate's `prompt` `<param>`; now `prior_episodes` and `standing_directives` each do too, and one over it fails the stage as `the reasoning could not run (error.execution)`. The original's 262144-byte projection budget can only be reached by the inputs together.
5. **A null confidence never escalates.** The original observes only a confidence that is not `None`, and `should_escalate(None)` is false; intuition's JavaScript read `null / 100 < threshold` as escalating. The decision service always gives each selection a confidence, so this only changes the case of no numeric confidence.
6. **Intuition's log names the decision, not the threshold.** `<intuition:escalate>` writes a boolean as the plan proposed (3.5), so the log is `turn N: <events> (confidence C, escalate B)`; the original logs neither. `after_the_warmup_the_threshold_follows_the_tuner` now proves the adaptive threshold by a confidence of 80 escalating after the warmup.
7. **`tuner` is `confidence_tuner` everywhere it is passed.** The plan names the datum; the intuition's and the cognition's datum, the `<param>` and the donedata field are renamed together. The log label `cognition.tuner` is kept.
8. **A tuner state that is not one reads as the initial state**, as `<decision:tuner>` did (the original's fresh `ConfidenceTuner()`).
9. **J2 is folded into J1's element**, as the plan's "one element is simpler" offered; J32's `environmentField` (the body's model snapshot) stays in `floppy.js` for a later `environment` step.
10. **JavaScript quirks are not reproduced.** For malformed input (a `self` or device `attributes` that is a string, which ECMAScript's `Object()` turns into indexed characters) the element renders nothing; well-formed input renders identically.
11. **Leftovers of step 1 fixed on the way.** `tests/tests/namespace_irp.rs` listed the processing suite but never ran it (it now does, with intuition and cognition); README.md's layout and `web/src/site-data.js` still named the removed decision elements.

## To review (step 3)

Choices step 3 (2026-10-18, D115) made that the plan did not settle, each
the closest to the original found, each for the maintainer to review:

1. **`<conversation:message_for_turn>` lost `keylocation`.** The relationship context reference is now worked out by `<conversation:relationship_for_input>`, where the original's `_relationship_for_input` calls `_stable_context_ref`; the conversation's first `commit` of each input existed only to compute it, so it is gone, and `context_ref` with it.
2. **`identities_json` is not an element.** No document needs it alone: the only caller was the memory row's JSON, and the original's `Memory.to_json` calls it, so it is part of `<memory:to_json>`. `<identity:sorted_identities>` is the only identity element J18 left.
3. **J4 is `<memory:contents location result>`.** The plan's `episodes_from_contents` and `directives_from_contents` are not in the original's tree; its memory has `OutputData.contents(statement_index)`, whose readers (`names_from_output`) validate each content and skip the rest. The element takes one statement's result (`_event.data.results[0]`) rather than the output and an index, so only that result crosses the extension boundary, and it parses each content as a JSON object, as reasoning's JavaScript did.
4. **J21 is two elements and two `<assign>`s.** `<conversation:participant_for_source>` (the original's name) adds a participant on first sight; `<conversation:fail_turn>` (after `_fail_turn`, which keeps only participants with a detector) drops the one whose detector failed. A failed remember restores the relationship's participants from a `participants` datum taken at the input's start (the original's `rollback` to `original_participants`) instead of filtering out an `added` list (the same participants, since only appends happen in between), and an inbound commit appends its sequence with `<assign location="relationships[relationship_index].sequences[...length]">`. Both are plain SCXML.
5. **J25 (`isCommittedAppend`) stays in `conversation.js`.** An element would have to read the whole history, which by the conversation's design never crosses the extension boundary (its values are bounded at 64 KiB on wasm).
6. **J26's names are `<conversation:ingest_content>` and `<conversation:on_end>`**, after `TurnDetector._ingest_content` and `_on_end`; the plan left the name open, and `on_` is a handler prefix, not a modal one like `should_`, so it is kept. The detector's six content data (`text_parts`, `audio`, `audio_sample_rate_hz`, `audio_channels`, `content`, `content_type`) are one `turn_content` datum, which `ingest_content` reads and writes; `on_end` takes the turn's references as four attributes beside it.
7. **`<conversation:new_track_ref>` runs in `turn_exchange`'s entry** (an `<invoke>`'s params cannot call an element), so on the state's last re-entry, when every source has had its turn, its missing source writes `null`. It sums the squares in order and writes the unit vector as ECMAScript's `JSON.stringify`, as `trackRef` did, not as Python's `fsum` and `repr` (which the relationship context reference uses). The current original's `_new_track_ref` is gone (`track_ref=source_id`).
8. **The similarity threshold and margin are Rust constants** (0.85 and 0.05, `SIMILARITY_THRESHOLD` and `SIMILARITY_MARGIN`), as the `ConfidenceTuner`'s defaults are (step 2), no longer the conversation's data.
9. **Identities may still be embeddings.** The current original's `IdentityValue` is a name only ("a biometric vector is never an identity"); floppy ported its earlier conversation, which matched voice embeddings, so every identity element accepts an array of numbers, sorted and compared by its Python JSON.
10. **The relationships cross the extension boundary.** `<conversation:relationship_for_input>` and `<conversation:detector_refs>` read every relationship (64 KiB per value on wasm); a long conversation with many embedding participants could reach it and fail the input as `error.execution`, where the JavaScript had no bound. `participant_for_source` and `fail_turn` read one relationship.
11. **`<conversation:complete_turn>` takes the relationship's `session_ref`** as an attribute (the original's `data.provenance.session_ref`) and commits the product itself, as `_complete_turn` calls `_message_for_turn`.
12. **JavaScript quirks are not reproduced.** `memoryPayload()` wrote `"content":undefined` for an input without content (the schema requires content); `<memory:to_json>` writes `null`. A row whose JSON holds a lone surrogate escape or a number out of range parsed with `JSON.parse` but is skipped by `<memory:contents>`.
13. **Q9 is not exercised.** Every document still has functions (glue: string and object builders, conds that correlate), so D65 is unchanged.

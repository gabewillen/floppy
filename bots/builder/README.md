# builder

The builder bot builds SCXML documents (never Rust). It is itself an SCXML
machine, [`builder.scxml`](builder.scxml), used by invoking it and talking to
it with events (decision D9 in [../../docs/decisions.md](../../docs/decisions.md)):

```xml
<invoke id="builder" type="scxml" src="builder.scxml"/>
<send event="builder.request" target="#_builder">
  <content expr="({spec: 'a traffic light with a pedestrian button'})"/>
</send>
<transition event="builder.done" .../>
<transition event="builder.failed" .../>
```

## Contract

| Event | Direction | Data ([`schemas/`](schemas)) |
| --- | --- | --- |
| `builder.request` | to the builder | `{spec, constraints?, max_attempts?, model?}`: the specification, extra constraints (strings), the failure budget (1 to 8, default 3), the `generate` model (default `gpt-4o-mini`) |
| `builder.done` | to `#_parent` | `{scxml, schemas, tests, report}`: the built document, its input event schemas (`{event, schema}`), its test scripts, and the report (attempts, failures, fixes applied, warnings, the plan, the triage decisions, the failure history, the simulation results) |
| `builder.failed` | to `#_parent` | `{reason, diagnostics}`: `invalid-request`, `attempts-exhausted`, or `abandoned`, with the last diagnostics (`{kind, code, message}`) |

The builder answers exactly once and then finishes. `intake` has no timeout:
the invoker owns the builder's lifetime (cancel the invoke to stop it).

`builder.scxml` carries the contracts as inline `event:schema` attributes:
an invoked child is compiled at run time and receives no schema files (a
schema reference would reject it), so `schemas/*.json` are the canonical
files and the `builder_bot` test of `nemobot-host` keeps the inline copies
equal to them. The engine treats them as metadata only; `intake` checks the
request with its `cond` and answers `builder.failed` (`invalid-request`)
otherwise.

## Topology

```
intake -> design -> author -> validate -> simulate -> package -> done
                       ^          |  ^        |
                       |     apply_fixes      |
                       |          v           v
                       +--- triage <- retry_gate -> failed
                            fallback
```

* `design`: `gen:generate` with the `topology_plan` structured-output schema.
* `author`: `gen:generate` with the `scxml_draft` schema: the document (plain
  SCXML with inline `event:schema` on its input transitions), the input event
  schemas as JSON text, and one to eight test scripts (input events with
  delays, a settle time, the outputs expected in order, and whether the
  machine finishes). A repair sees the failed draft, its tests, and the
  diagnostics.
* `validate`: the draft's schemas must parse; `sx:validate` with
  `applyfixes="machine-applicable"`; when the fixes change the text,
  `apply_fixes` takes them and validates again (at most 2 passes per draft);
  otherwise the error diagnostics feed back to `author`.
* `simulate`: each test script runs the draft as an invoked child
  (`<invoke id="candidate" type="scxml"><content expr="scxml_text"/></invoke>`),
  sends the script's events through `#_candidate` with their delays, collects
  every event the child sends to `#_parent` until the settle time (or
  `done.invoke.candidate`), and compares: the expected outputs must appear in
  order (others may come between) and the child must finish exactly when the
  test says so.
* `package`: sends `builder.done`.
* `retry_gate`: every failure (generation, draft, validation, simulation,
  timeout) passes through it; it counts the failure and fails the build at
  `max_attempts`, so every loop is bounded.
* `triage`: `decision:choose` picks among the state's enabled `event:schema`
  transitions, `triage.repair`, `triage.redesign`, and `triage.abandon`.
* `fallback`: the cond-based triage when the decision is unavailable (no
  `TYPESAFE_API_KEY` granted or a backend failure: `error.execution`) or
  abstains (`decision.abstain`): redesign without a plan, else repair.

Timeouts are delayed sends cancelled on exit (`design` 180 s, `author`
300 s, `simulate` 900 s); a timeout is a failure like any other.

## Bindings

A session that runs the builder binds the `scxml`, `generate`, and `decision`
namespace packages, an `extension-call-fuel` large enough for `sx:validate`
(1 billion; see `namespaces/scxml`), and grants `OPENAI_API_KEY` (and
optionally `OPENAI_BASE_URL`) for `generate` and `TYPESAFE_API_KEY` (and
optionally `TYPESAFE_BASE_URL`) for `decision`. Drafts are validated without
packages, so a built machine uses no extension namespace or service.

## Proof

`host/tests/builder_bot.rs` invokes the builder from a parent machine with
the spec "a traffic light with a pedestrian button" against a fake
Chat Completions and `system_one` backend on 127.0.0.1: an invalid first
draft (`SCXML005`) goes back to `author` through `triage.repair`, the second
draft's only error is fixed by its machine-applicable fix, and the fixed
draft simulates as an invoked child before `builder.done`. Other cases cover
the fallback triage exhausting `max_attempts` and an invalid request. The
ignored `live_builder_through_an_env_file` test runs the same spec with the
real backends; [`examples/traffic_light`](examples/traffic_light) is one such
live build (`gpt-4.1`; it passed on its first attempt, so no triage ran; an
earlier live run repaired twice through TypeSafe's `triage.repair`), kept
because it validates and passed its own simulation. It is model output, kept as built: for example
its `onexit` handlers do not cancel the phase timers, which its tests do not
detect.

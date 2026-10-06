# builder

The builder bot builds SCXML documents (never Rust). It is itself an SCXML
machine, used by invoking it (`<invoke type="scxml" src="...">`) and talking
to it with events: `builder.request` in, `builder.done` or `builder.failed`
out. Its documents land here in a later change; the loop it will implement is
decision D9 in [../../docs/decisions.md](../../docs/decisions.md).

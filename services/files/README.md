# files

The `files` invoked service: reads and writes the files of one directory the
run grants, as a Wasm component
(`https://github.com/stateforward/floppy/services/files`, component
`floppy_service_files`). The builder bot
([`bots/builder/builder.scx`](../../bots/builder/builder.scx)) writes each
draft through it as three documents beside each other, `<name>.scx`,
`<name>.json` and `<name>.js`, and reflection
([`abilities/reflection/reflection.scx`](../../abilities/reflection/reflection.scx))
reads a stored behavior's back through it to revise it (decision D121). Its
crate documentation ([src/lib.rs](src/lib.rs)) is the full contract.

- **Model.** scxml.rs's files example service
  ([`scxml.rs/components/scxml-service-files`](../../scxml.rs/components/scxml-service-files)),
  with its parameter names (`name` and `text` to write, `seed` to read) and
  its result: the done data `{"seed": TEXT-OR-NULL, "wrote": BYTES-OR-NULL}`,
  no event. An invalid parameter or a failed filesystem call rejects the
  start (`error.execution`).
- **Grant.** The directory is the run's
  (`--dir https://github.com/stateforward/floppy/services/files=HOST::/behaviors`);
  a run that binds the service without it is refused before it starts. HOST
  is the `behaviors/` directory beside `bots/` and `abilities/`, where the
  documents that invoke a learned behavior find it
  (`../../behaviors/<name>.scx`).
- **Names.** A name is one plain path component: a name with `/` or `\`,
  `.`, `..`, or a control character is refused, so nothing is read or
  written outside the directory.
- **WASI.** The component's `wasi:cli` imports are satisfied by its
  model's composed deny-all provider
  ([`scxml.rs/components/scxml-service-files/wasi-cli-deny.wat`](../../scxml.rs/components/scxml-service-files/wasi-cli-deny.wat)),
  and it keeps exactly the filesystem it imports for the host.

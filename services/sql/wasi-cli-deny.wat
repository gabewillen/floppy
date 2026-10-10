;; Deny-all `wasi:cli` (and monotonic-clock) provider composed into the
;; sql service before release (see scripts/build-components.sh), and into
;; the generate service, which keeps `wasi:filesystem` the same way to read
;; the images its messages name.
;;
;; Adapted from scxml.rs's files example service
;; (scxml.rs/components/scxml-service-files/wasi-cli-deny.wat): the same
;; functions, plus the monotonic clock's `subscribe-instant` (wasi-libc
;; imports it beside `subscribe-duration`; it traps too), at the WASI
;; 0.2.12 interface versions this component imports (its WIT world's
;; packages raise the standard library's 0.2.9 imports to 0.2.12 when the
;; component is encoded).
;;
;; The sql service keeps `wasi:filesystem` (and the `wasi:io` and
;; `wasi:clocks/wall-clock` interfaces SQLite reaches through it) for the
;; host, which links them only under a `--dir` grant; a wasi-virt adapter
;; would virtualize the filesystem too (its `--allow-fs` drops the
;; filesystem imports), so this provider satisfies the standard library's
;; and wasi-libc's other WASI imports instead.
;;
;; Every function denies: the standard streams, exit, and the monotonic
;; clock's subscription trap; the environment is empty; no stream is a
;; terminal. The service never reaches any of them on its paths: it prints
;; nothing, reads no variable, and never sleeps (SQLite's busy timeout is
;; zero, so a locked database fails at once instead of waiting). This is
;; declarative data: it reaches no host capability and executes no SCXML
;; behavior; its only imports are the `wasi:io` resource types the
;; interfaces it exports name, which compose into the component's own
;; `wasi:io` imports.
;;
;; The interface versions must match the ones the component imports; a
;; mismatch leaves an import unsatisfied, and the import allowlist check
;; fails closed.
(component
  (import "wasi:io/poll@0.2.12" (instance $poll
    (export "pollable" (type (sub resource)))))
  (alias export $poll "pollable" (type $pollable))
  (import "wasi:io/streams@0.2.12" (instance $streams
    (export "input-stream" (type (sub resource)))
    (export "output-stream" (type (sub resource)))))
  (alias export $streams "input-stream" (type $input-stream))
  (alias export $streams "output-stream" (type $output-stream))

  (type $terminal-input (resource (rep i32)))
  (type $terminal-output (resource (rep i32)))

  (core module $deny
    (memory (export "memory") 1)
    ;; Offset 8 holds 16 zero bytes: an empty list (pointer 0, length 0)
    ;; and `none` (discriminant 0).
    (func (export "trap-stream") (result i32) unreachable)
    (func (export "trap-exit") (param i32) unreachable)
    (func (export "trap-subscribe") (param i64) (result i32) unreachable)
    (func (export "zeroes") (result i32) (i32.const 8)))
  (core instance $instance (instantiate $deny))

  (func $get-stdin (result (own $input-stream))
    (canon lift (core func $instance "trap-stream")))
  (func $get-stdout (result (own $output-stream))
    (canon lift (core func $instance "trap-stream")))
  (func $get-stderr (result (own $output-stream))
    (canon lift (core func $instance "trap-stream")))
  (func $get-environment (result (list (tuple string string)))
    (canon lift (core func $instance "zeroes") (memory $instance "memory")))
  (func $exit (param "status" (result))
    (canon lift (core func $instance "trap-exit")))
  (func $get-terminal-stdin (result (option (own $terminal-input)))
    (canon lift (core func $instance "zeroes") (memory $instance "memory")))
  (func $get-terminal-stdout (result (option (own $terminal-output)))
    (canon lift (core func $instance "zeroes") (memory $instance "memory")))
  (func $get-terminal-stderr (result (option (own $terminal-output)))
    (canon lift (core func $instance "zeroes") (memory $instance "memory")))
  (type $instant u64)
  (type $duration u64)
  (func $subscribe-instant (param "when" $instant) (result (own $pollable))
    (canon lift (core func $instance "trap-subscribe")))
  (func $subscribe-duration (param "when" $duration) (result (own $pollable))
    (canon lift (core func $instance "trap-subscribe")))

  (instance $stdin
    (export "input-stream" (type $input-stream))
    (export "get-stdin" (func $get-stdin)))
  (instance $stdout
    (export "output-stream" (type $output-stream))
    (export "get-stdout" (func $get-stdout)))
  (instance $stderr
    (export "output-stream" (type $output-stream))
    (export "get-stderr" (func $get-stderr)))
  (instance $environment
    (export "get-environment" (func $get-environment)))
  (instance $exit-interface
    (export "exit" (func $exit)))
  (instance $terminal-input-interface
    (export "terminal-input" (type $terminal-input)))
  (instance $terminal-output-interface
    (export "terminal-output" (type $terminal-output)))
  (instance $terminal-stdin
    (export "terminal-input" (type $terminal-input))
    (export "get-terminal-stdin" (func $get-terminal-stdin)))
  (instance $terminal-stdout
    (export "terminal-output" (type $terminal-output))
    (export "get-terminal-stdout" (func $get-terminal-stdout)))
  (instance $terminal-stderr
    (export "terminal-output" (type $terminal-output))
    (export "get-terminal-stderr" (func $get-terminal-stderr)))
  (instance $monotonic-clock
    (export "pollable" (type $pollable))
    (export "instant" (type $instant))
    (export "duration" (type $duration))
    (export "subscribe-instant" (func $subscribe-instant))
    (export "subscribe-duration" (func $subscribe-duration)))

  (export "wasi:cli/stdin@0.2.12" (instance $stdin))
  (export "wasi:cli/stdout@0.2.12" (instance $stdout))
  (export "wasi:cli/stderr@0.2.12" (instance $stderr))
  (export "wasi:cli/environment@0.2.12" (instance $environment))
  (export "wasi:cli/exit@0.2.12" (instance $exit-interface))
  (export "wasi:cli/terminal-input@0.2.12" (instance $terminal-input-interface))
  (export "wasi:cli/terminal-output@0.2.12" (instance $terminal-output-interface))
  (export "wasi:cli/terminal-stdin@0.2.12" (instance $terminal-stdin))
  (export "wasi:cli/terminal-stdout@0.2.12" (instance $terminal-stdout))
  (export "wasi:cli/terminal-stderr@0.2.12" (instance $terminal-stderr))
  (export "wasi:clocks/monotonic-clock@0.2.12" (instance $monotonic-clock)))

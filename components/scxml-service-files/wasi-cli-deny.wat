;; Deny-all `wasi:cli` (and monotonic-clock) provider composed into the files
;; example service before release (see scripts/verify-wasm-capabilities.sh).
;;
;; The component declares exactly one WASI permission, `wasi:filesystem`
;; (package/files.service.json), but Rust's standard library also imports
;; the `wasi:cli` interfaces and `wasi:clocks/monotonic-clock`. The other
;; components strip those with wasi-virt, but a wasi-virt adapter
;; virtualizes the filesystem too (read-only), so this component composes
;; this provider instead and keeps its filesystem imports for the host.
;;
;; Every function denies: the standard streams, exit, and the monotonic
;; clock's subscription trap; the environment is empty; no stream is a
;; terminal. The component never reaches any of them on its paths (it
;; prints nothing, reads no variable, and never sleeps). This is
;; declarative data: it reaches no host capability and executes no SCXML
;; behavior; its only imports are the `wasi:io` resource types the
;; interfaces it exports name, which compose into the component's own
;; `wasi:io` imports.
;;
;; The interface versions must match the ones the pinned Rust standard
;; library imports; a mismatch leaves an import unsatisfied, and the import
;; allowlist check fails closed.
(component
  (import "wasi:io/poll@0.2.9" (instance $poll
    (export "pollable" (type (sub resource)))))
  (alias export $poll "pollable" (type $pollable))
  (import "wasi:io/streams@0.2.9" (instance $streams
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
  (type $duration u64)
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
    (export "duration" (type $duration))
    (export "subscribe-duration" (func $subscribe-duration)))

  (export "wasi:cli/stdin@0.2.9" (instance $stdin))
  (export "wasi:cli/stdout@0.2.9" (instance $stdout))
  (export "wasi:cli/stderr@0.2.9" (instance $stderr))
  (export "wasi:cli/environment@0.2.9" (instance $environment))
  (export "wasi:cli/exit@0.2.9" (instance $exit-interface))
  (export "wasi:cli/terminal-input@0.2.9" (instance $terminal-input-interface))
  (export "wasi:cli/terminal-output@0.2.9" (instance $terminal-output-interface))
  (export "wasi:cli/terminal-stdin@0.2.9" (instance $terminal-stdin))
  (export "wasi:cli/terminal-stdout@0.2.9" (instance $terminal-stdout))
  (export "wasi:cli/terminal-stderr@0.2.9" (instance $terminal-stderr))
  (export "wasi:clocks/monotonic-clock@0.2.9" (instance $monotonic-clock)))

;; Deterministic `wasi:random/random` and `wasi:random/insecure-seed`
;; provider composed into the validator and compiler components (the
;; compiler links the same validator) before wasi-virt (see
;; scripts/verify-wasm-capabilities.sh).
;;
;; The bundled XML Schema backend (`xsd-schema`) keys its name tables with
;; `ahash`, whose default `runtime-rng` feature reads
;; `random.get-random-u64` once per instance for its hash seeds, and std's
;; `RandomState` (the `event` vocabulary's boon schema checks, for one) reads
;; `insecure-seed`. wasi-virt denies ambient randomness, so without a
;; provider the first schema validation, or the first event schema, traps.
;; The values only key in-memory hash tables; they never reach a diagnostic
;; or the canonical report, and fixed values keep every validation
;; byte-deterministic. The
;; validator bounds every request (16 MiB of source, 64 schemas of 4 MiB),
;; so hash flooding is bounded too. This is declarative data: it exports
;; constants and executes no SCXML behavior.
;;
;; The value is "SCXMLvld" read as a little-endian u64; the seed pair is
;; "SCXML-validator\0" read as two little-endian u64 values. The interface
;; version must match the one the pinned dependencies import; a mismatch
;; leaves the import unsatisfied, and the import allowlist check fails
;; closed.
(component
  (core module $random
    (memory (export "memory") 1)
    (data (i32.const 16) "\53\43\58\4d\4c\2d\76\61\6c\69\64\61\74\6f\72\00")
    (func (export "get-random-u64") (result i64) (i64.const 0x646c764c4d584353))
    (func (export "insecure-seed") (result i32) (i32.const 16)))
  (core instance $instance (instantiate $random))
  (func $get-random-u64 (result u64)
    (canon lift (core func $instance "get-random-u64")))
  (func $insecure-seed (result (tuple u64 u64))
    (canon lift (core func $instance "insecure-seed") (memory $instance "memory")))
  (instance $random-interface (export "get-random-u64" (func $get-random-u64)))
  (instance $seed-interface (export "insecure-seed" (func $insecure-seed)))
  (export "wasi:random/random@0.2.12" (instance $random-interface))
  (export "wasi:random/insecure-seed@0.2.12" (instance $seed-interface)))

;; Deterministic `wasi:random/insecure-seed` provider composed into the
;; ECMAScript data-model component before wasi-virt (see
;; scripts/verify-wasm-capabilities.sh).
;;
;; Boa's `Map`, `Set`, and object-shape tables use std's `RandomState`,
;; whose first use reads `insecure-seed` once per instance to key SipHash.
;; wasi-virt denies ambient randomness, so without a provider that read
;; traps. The seed only keys hash tables that iterate in insertion order;
;; it is never observable by expressions, and a fixed seed keeps every
;; evaluation byte-deterministic. Inputs are bounded by the data model, so
;; hash flooding is not a concern. This is declarative data: it exports a
;; constant and executes no SCXML behavior.
;;
;; The seed is the 16 bytes "SCXML-seed-v1\0\0\0" read as two little-endian
;; u64 values. The interface version must match the one the pinned Rust
;; standard library imports; a mismatch leaves the import unsatisfied, and
;; the import allowlist check fails closed.
(component
  (core module $seed
    (memory (export "memory") 1)
    (data (i32.const 16) "\53\43\58\4d\4c\2d\73\65\65\64\2d\76\31\00\00\00")
    (func (export "insecure-seed") (result i32) (i32.const 16)))
  (core instance $instance (instantiate $seed))
  (func $insecure-seed (result (tuple u64 u64))
    (canon lift (core func $instance "insecure-seed") (memory $instance "memory")))
  (instance $interface (export "insecure-seed" (func $insecure-seed)))
  (export "wasi:random/insecure-seed@0.2.9" (instance $interface)))

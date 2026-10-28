//! Property tests at the `scxml-types` trust boundaries: the canonical
//! `<send delay>` parser and the delayed-send wire decoder.
//!
//! Runs are deterministic: a fixed RNG seed, a fixed case count, and no
//! failure-persistence files, so identical sources give identical runs.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use scxml_types::{
    BaggagePolicy, DataValue, DelayError, DelayedSend, Event, EventName, EventType,
    MAX_DELAY_BYTES, MAX_DELAYED_SEND_BYTES, NamedValue, ProcessorSend, SendId, SendRequest,
    TraceContext, decode_delayed_send, encode_delayed_send, parse_delay_ms,
};

fn config() -> Config {
    Config {
        cases: 512,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x5c3a_1d07),
        ..Config::default()
    }
}

/// The event I/O processor a fixture delayed send targets.
#[derive(Clone, Copy, Debug)]
enum Processor {
    Scxml,
    BasicHttp,
}

fn processor() -> impl Strategy<Value = Processor> {
    prop_oneof![Just(Processor::Scxml), Just(Processor::BasicHttp)]
}

fn valid_request(delay_ms: u64, processor: Processor) -> DelayedSend {
    if let Processor::BasicHttp = processor {
        return DelayedSend::Processor(ProcessorSend {
            processor_type: "basichttp".into(),
            send_id: SendId::new("timer-1").expect("fixture send id is valid"),
            event_name: Some("order.ready".into()),
            target: Some("http://example.test/in".into()),
            params: vec![NamedValue {
                name: "k".into(),
                value: DataValue::String("v".into()),
            }],
            content: None,
            delay_ms,
            trace_context: None,
        });
    }
    DelayedSend::Scxml(SendRequest {
        send_id: Some(SendId::new("timer-1").expect("fixture send id is valid")),
        event: Event {
            name: EventName::new("order.ready").expect("fixture event name is valid"),
            event_type: EventType::External,
            send_id: Some(SendId::new("timer-1").expect("fixture send id is valid")),
            origin: Some("#_internal".into()),
            origin_type: Some("scxml".into()),
            invoke_id: None,
            data: DataValue::JsonObject("{\"ok\":true}".into()),
            trace_context: Some(TraceContext {
                traceparent: "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01".into(),
                tracestate: None,
                baggage: None,
                baggage_policy: BaggagePolicy::Drop,
            }),
            raw: None,
        },
        target: Some("#_internal".into()),
        processor_type: Some("scxml".into()),
        delay_ms,
        message: scxml_types::SendMessage::Empty,
    })
}

proptest! {
    #![proptest_config(config())]

    /// Any string is either a typed rejection or a value; the parser never
    /// panics and enforces its byte bound before inspecting content.
    #[test]
    fn delay_parser_is_total(value in any::<String>()) {
        let result = parse_delay_ms(&value);
        if value.len() > MAX_DELAY_BYTES {
            prop_assert_eq!(result, Err(DelayError::TooLong));
        }
    }

    /// Whole-number designations map exactly onto milliseconds.
    #[test]
    fn whole_delays_are_exact(whole in any::<u32>(), plus in any::<bool>()) {
        let sign = if plus { "+" } else { "" };
        prop_assert_eq!(parse_delay_ms(&format!("{sign}{whole}ms")), Ok(u64::from(whole)));
        prop_assert_eq!(
            parse_delay_ms(&format!("{sign}{whole}s")),
            Ok(u64::from(whole) * 1_000)
        );
    }

    /// Fractional seconds floor to whole milliseconds: only the first three
    /// fraction digits contribute.
    #[test]
    fn fractional_seconds_floor(whole in 0_u32..1_000_000, fraction in "[0-9]{1,12}") {
        let mut expected = u64::from(whole) * 1_000;
        for (digit, weight) in fraction.bytes().zip([100_u64, 10, 1]) {
            expected += u64::from(digit - b'0') * weight;
        }
        prop_assert_eq!(parse_delay_ms(&format!("{whole}.{fraction}s")), Ok(expected));
        prop_assert_eq!(parse_delay_ms(&format!(".{fraction}s")), Ok(expected - u64::from(whole) * 1_000));
    }

    /// A leading `-` is always rejected as negative, never as a value.
    #[test]
    fn negative_delays_are_rejected(whole in any::<u32>()) {
        prop_assert_eq!(parse_delay_ms(&format!("-{whole}ms")), Err(DelayError::Negative));
    }

    /// Arbitrary bytes never panic the delayed-send decoder, and anything it
    /// accepts re-encodes to exactly the same canonical bytes.
    #[test]
    fn send_decoder_is_total_and_canonical(
        bytes in proptest::collection::vec(any::<u8>(), 0..512)
    ) {
        if let Ok(send) = decode_delayed_send(&bytes) {
            prop_assert_eq!(encode_delayed_send(&send), Ok(bytes));
        }
    }

    /// Single-byte corruption of a valid encoding is either rejected or
    /// decodes to a request whose canonical encoding is the corrupted bytes.
    #[test]
    fn corrupted_send_requests_fail_closed(
        delay_ms in any::<u64>(),
        processor in processor(),
        index in any::<prop::sample::Index>(),
        replacement in any::<u8>(),
    ) {
        let request = valid_request(delay_ms, processor);
        let mut bytes = encode_delayed_send(&request).expect("fixture request encodes");
        prop_assert_eq!(decode_delayed_send(&bytes), Ok(request));
        let position = index.index(bytes.len());
        bytes[position] = replacement;
        if let Ok(decoded) = decode_delayed_send(&bytes) {
            prop_assert_eq!(encode_delayed_send(&decoded), Ok(bytes));
        }
    }

    /// Truncating a valid encoding anywhere is always rejected.
    #[test]
    fn truncated_send_requests_are_rejected(
        delay_ms in any::<u64>(),
        processor in processor(),
        index in any::<prop::sample::Index>(),
    ) {
        let bytes = encode_delayed_send(&valid_request(delay_ms, processor))
            .expect("fixture request encodes");
        let cut = index.index(bytes.len());
        prop_assert!(decode_delayed_send(&bytes[..cut]).is_err());
    }
}

#[test]
fn oversized_send_requests_are_rejected_before_decoding() {
    let bytes = vec![0_u8; MAX_DELAYED_SEND_BYTES + 1];
    assert!(decode_delayed_send(&bytes).is_err());
}

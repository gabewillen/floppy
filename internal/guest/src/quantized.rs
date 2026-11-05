//! The compact text of a quantized vector: `i8:` and the standard base64
//! (padded) of its components, each `round(component * 127)` as a signed
//! byte. The conversation namespace writes a person's representative voice
//! as one, and the memory namespace's episode scoring reads the vectors
//! reflection stores, so the codec lives here once.
//!
//! A 256-dimensional vector is 347 characters, where its JSON text is about
//! 5 KiB. Quantizing keeps a 256-dimensional vector's cosine with the
//! vector it came from above 0.999 (each component moves by at most
//! 1/254), so a match moves by about a thousandth, against D90's margin of
//! 0.05.

/// The prefix of a compact quantized vector's text: signed bytes.
pub const PREFIX: &str = "i8:";

/// The scale of a quantized component: a unit component times this,
/// rounded.
pub const SCALE: f64 = 127.0;

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A quantized vector's components (each signed byte over [`SCALE`]), from
/// its compact text; `None` for text of another form: not prefixed, empty,
/// of a length that is not a whole number of base64 characters, padded in
/// the middle, padded over two characters, or holding a character outside
/// the alphabet.
///
/// A vector whose components are all zero decodes to them (a caller that
/// refuses it filters it); a vector of another length than the one
/// compared against is the caller's mismatch, not a damaged text.
#[must_use]
pub fn decode(text: &str) -> Option<Vec<f64>> {
    let body = text.strip_prefix(PREFIX)?.as_bytes();
    if body.is_empty() || body.len() % 4 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(body.len() / 4 * 3);
    let quads = body.len() / 4;
    for (index, quad) in body.chunks(4).enumerate() {
        let last = index + 1 == quads;
        let padding = quad.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return None;
        }
        let mut sextets = [0_u8; 4];
        for (slot, byte) in sextets.iter_mut().zip(quad.iter().take(4 - padding)) {
            let position = BASE64.iter().position(|known| known == byte)?;
            *slot = u8::try_from(position).ok()?;
        }
        let decoded = [
            (sextets[0] << 2) | (sextets[1] >> 4),
            (sextets[1] << 4) | (sextets[2] >> 2),
            (sextets[2] << 6) | sextets[3],
        ];
        bytes.extend_from_slice(&decoded[..3 - padding]);
    }
    Some(
        bytes
            .iter()
            .map(|byte| f64::from(i8::from_le_bytes([*byte])) / SCALE)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn decodes_the_known_texts() {
        let decoded = decode("i8:TGY=").expect("it decodes");
        assert_eq!(decoded, [76.0 / 127.0, 102.0 / 127.0]);
        // The unit vector of [3, 0, -4] quantized: the encoding
        // namespaces/conversation ships normalizes first.
        assert_eq!(
            decode("i8:TACa").expect("it decodes"),
            [76.0 / 127.0, 0.0, -102.0 / 127.0]
        );
        assert_eq!(decode("i8:fw==").expect("it decodes"), [127.0 / 127.0]);
        assert_eq!(decode("i8:AAA="), Some(vec![0.0, 0.0]));
    }

    #[test]
    fn refuses_other_forms() {
        for damaged in [
            "",
            "i8:",
            "TGU=",
            "i8:TGU",
            "i8:T=U=",
            "i8:TG!=",
            "f16:TGU=",
            "i8:AAAA=A",
            "i8:A===",
        ] {
            assert_eq!(decode(damaged), None, "{damaged}");
        }
    }

    #[test]
    fn decodes_a_256_dimensional_vector() {
        // 256 quantized bytes: 85 three-byte groups, the last holding one
        // byte.
        let body = "QQOA".repeat(85) + "QQ==";
        let text = format!("i8:{body}");
        assert_eq!(text.len(), 347);
        let components = decode(&text).expect("it decodes");
        assert_eq!(components.len(), 256);
        // The quantized bytes, within the doubles' rounding.
        for (index, expected) in [0, 1, 255]
            .iter()
            .zip([65.0 / 127.0, 3.0 / 127.0, 65.0 / 127.0])
        {
            assert!(
                (components[*index] - expected).abs() < 1e-15,
                "{index}: {}",
                components[*index]
            );
        }
    }
}

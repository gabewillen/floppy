//! People in long-term memory (decisions D194 and D205): matching a voice
//! to a person, a person's representative voice, and the consolidation
//! rest does on them.
//!
//! A person's representative voice is one compact text per person
//! ([`encode_voice`]): the unit vector of the voices folded into it,
//! each component quantized to a signed byte (`round(component * 127)`)
//! and written as `i8:` and the bytes' standard base64. A 256-dimensional
//! voice is 347 characters, where its JSON text is about 5 KiB, so reading
//! one representative per person is about 350 bytes a person, and nothing
//! that carries them has a size bound (decision D271).
//! Quantizing keeps a
//! 256-dimensional voice's cosine with the voice it came from above 0.999
//! (each component moves by at most 1/254), so a match moves by about a
//! thousandth, against D90's margin of 0.05.

use serde_json::{Map, Value as Json};

use super::{
    Attribute, Content, ConversationHost, Data, Executed, Failure, SIMILARITY_MARGIN,
    SIMILARITY_THRESHOLD, VOICE_SIMILARITY_THRESHOLD, assign_to, attribute_values,
    cosine_similarity, embedding, evaluate, identities_of, is_identity, no_content, read_json,
    relationship_context_ref, unit_vector,
};

/// The prefix of a person's participant ref, `person-<n>`.
pub const PERSON_REF_PREFIX: &str = "person-";

/// The prefix of a representative voice's compact text: signed bytes.
pub const VOICE_PREFIX: &str = "i8:";

/// The scale of a quantized component: a unit component times this,
/// rounded.
const VOICE_SCALE: f64 = 127.0;

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A voice's compact text: `i8:` and the standard base64 (padded) of its
/// unit vector's components, each `round(component * 127)` as a signed
/// byte; `None` for a voice without a norm (empty, zero, or not finite).
#[must_use]
pub fn encode_voice(voice: &[f64]) -> Option<String> {
    let unit = unit_vector(voice);
    if unit.is_empty() || !unit.iter().all(|component| component.is_finite()) {
        return None;
    }
    let norm: f64 = unit.iter().map(|component| component * component).sum();
    if (norm - 1.0).abs() > 1e-6 {
        return None;
    }
    let bytes: Vec<u8> = unit
        .iter()
        .map(|component| {
            let quantized = (component * VOICE_SCALE)
                .round()
                .clamp(-VOICE_SCALE, VOICE_SCALE);
            #[allow(
                clippy::cast_possible_truncation,
                reason = "a whole number from -127 to 127 converts exactly"
            )]
            let byte = quantized as i8;
            byte.to_le_bytes()[0]
        })
        .collect();
    let mut text = String::with_capacity(VOICE_PREFIX.len() + bytes.len().div_ceil(3) * 4);
    text.push_str(VOICE_PREFIX);
    for chunk in bytes.chunks(3) {
        let [a, b, c] = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let sextets = [
            a >> 2,
            ((a & 0x03) << 4) | (b >> 4),
            ((b & 0x0f) << 2) | (c >> 6),
            c & 0x3f,
        ];
        for (index, sextet) in sextets.iter().enumerate() {
            if index <= chunk.len() {
                text.push(char::from(BASE64[usize::from(*sextet)]));
            } else {
                text.push('=');
            }
        }
    }
    Some(text)
}

/// A representative voice's components (each signed byte over 127), from
/// its compact text; `None` for text of another form (a damaged row).
#[must_use]
pub fn decode_voice(text: &str) -> Option<Vec<f64>> {
    let body = text.strip_prefix(VOICE_PREFIX)?.as_bytes();
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
    let components: Vec<f64> = bytes
        .iter()
        .map(|byte| f64::from(i8::from_le_bytes([*byte])) / VOICE_SCALE)
        .collect();
    components
        .iter()
        .any(|component| *component != 0.0)
        .then_some(components)
}

/// A whole number from 0 to 2^53.
fn whole_number(value: f64) -> Option<u64> {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if value.fract() == 0.0 && (0.0..EXACT).contains(&value) {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a whole number from 0 to 2^53 converts exactly"
        )]
        return Some(value as u64);
    }
    None
}

/// Evaluates `expression` to a whole number, else `malformed`.
fn read_whole<H: ConversationHost>(
    host: &mut H,
    expression: &str,
    [failed, malformed]: [&'static str; 2],
) -> Result<u64, Failure<H::Error>> {
    match evaluate(host, expression, failed)? {
        Data::Number(value) => whole_number(value.get()),
        _ => None,
    }
    .ok_or(Failure::Execution(malformed))
}

/// Evaluates `expression` to an array, else `malformed`.
fn read_array<H: ConversationHost>(
    host: &mut H,
    expression: &str,
    details: [&'static str; 2],
) -> Result<Vec<Json>, Failure<H::Error>> {
    match read_json(host, expression, details)? {
        Json::Array(values) => Ok(values),
        _ => Err(Failure::Execution(details[1])),
    }
}

/// Evaluates `expression` to a non-empty string, else `malformed`.
fn read_text<H: ConversationHost>(
    host: &mut H,
    expression: &str,
    [failed, malformed]: [&'static str; 2],
) -> Result<String, Failure<H::Error>> {
    match evaluate(host, expression, failed)? {
        Data::String(text) if !text.is_empty() => Ok(text),
        _ => Err(Failure::Execution(malformed)),
    }
}

/// An embedding's components from its JSON text (a non-empty array of
/// numbers); `None` for text of another form.
fn voice_of_text(text: &str) -> Option<Vec<f64>> {
    let Ok(Json::Array(components)) = serde_json::from_str::<Json>(text) else {
        return None;
    };
    if components.is_empty() {
        return None;
    }
    components.iter().map(Json::as_f64).collect()
}

/// The representatives memory holds, `[person_ref, voice]` each (a
/// non-empty person ref and a representative's compact text), as
/// `(person_ref, components)`; an entry of another shape is a damaged
/// row, which matches no one, and is skipped.
fn known_voices(voices: &[Json]) -> Vec<(String, Vec<f64>)> {
    voices
        .iter()
        .filter_map(|voice| match voice.as_array().map(Vec::as_slice) {
            Some([Json::String(person), Json::String(text)]) if !person.is_empty() => {
                decode_voice(text).map(|components| (person.clone(), components))
            }
            _ => None,
        })
        .collect()
}

/// Executes `<conversation:people_for_input>`.
pub(super) fn people_for_input_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, input, voices, after] = attribute_values(
        attributes,
        ["location", "input", "voices", "after"],
        "conversation:people_for_input takes only the location, input, voices and after attributes",
        "conversation:people_for_input repeats an attribute",
        "conversation:people_for_input has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:people_for_input requires a location",
    ))?;
    let input = input.ok_or(Failure::Execution(
        "conversation:people_for_input requires an input",
    ))?;
    let voices = voices.ok_or(Failure::Execution(
        "conversation:people_for_input requires voices",
    ))?;
    let after = after.ok_or(Failure::Execution(
        "conversation:people_for_input requires after",
    ))?;
    no_content(content, "conversation:people_for_input holds no content")?;
    let Json::Object(input) = read_json(
        host,
        input,
        [
            "conversation:people_for_input input did not evaluate",
            "conversation:people_for_input input is not an object",
        ],
    )?
    else {
        return Err(Failure::Execution(
            "conversation:people_for_input input is not an object",
        ));
    };
    let voices = read_array(
        host,
        voices,
        [
            "conversation:people_for_input voices did not evaluate",
            "conversation:people_for_input voices is not an array",
        ],
    )?;
    let after = read_whole(
        host,
        after,
        [
            "conversation:people_for_input after did not evaluate",
            "conversation:people_for_input after is not a whole number",
        ],
    )?;
    let identified =
        people_for(&input, &known_voices(&voices), after).map_err(Failure::Execution)?;
    assign_to(
        host,
        location,
        &Data::JsonObject(identified.to_string()),
        "conversation:people_for_input could not assign the people",
    )?;
    Ok(Executed::Handled)
}

/// The person a voice is, among the `known` voices (`(person_ref,
/// embedding)`, in the order memory holds them), by the conversation's
/// embedding match (decision D90) at the voice threshold: a person scores
/// the highest cosine similarity of the voice with any of theirs, and is a
/// candidate at [`VOICE_SIMILARITY_THRESHOLD`] or more; the best candidate is the voice's
/// person unless another candidate comes within [`SIMILARITY_MARGIN`] of
/// it (as two identity groups' matches are refused), when the voice is no
/// one's.
#[must_use]
pub fn person_for_voice<'a>(voice: &[f64], known: &'a [(String, Vec<f64>)]) -> Option<&'a str> {
    let mut scores: Vec<(&str, f64)> = Vec::new();
    for (person, embedding) in known {
        let Some(score) = cosine_similarity(voice, embedding) else {
            continue;
        };
        match scores.iter_mut().find(|(known, _)| known == person) {
            Some((_, best)) => *best = best.max(score),
            None => scores.push((person, score)),
        }
    }
    let mut best: Option<(&str, f64)> = None;
    let mut second: Option<f64> = None;
    for (person, score) in scores {
        if score < VOICE_SIMILARITY_THRESHOLD {
            continue;
        }
        match best {
            Some((_, most)) if score <= most => {
                second = Some(second.map_or(score, |second| second.max(score)));
            }
            _ => {
                second = best.map(|(_, most)| most);
                best = Some((person, score));
            }
        }
    }
    let (person, most) = best?;
    if second.is_some_and(|second| most - second <= SIMILARITY_MARGIN) {
        return None;
    }
    Some(person)
}

/// What `<conversation:people_for_input>` makes of an input: `{input,
/// people, voices, representatives}`. Each embedding of the input's
/// `source_ids`, then of its `target_ids`, in order, is matched to a
/// person among the `known` representatives and the voices of the people
/// created before it ([`person_for_voice`]); a voice that is no one's is a
/// new person, `person-<n>`, numbered on after `after` (the last number of
/// the people sequence the invoker holds). `input` is the input with each
/// embedding replaced by its person's ref and each list without repeats
/// (the first kept); `people` the refs of the people created, in order;
/// `voices` `[person_ref, embedding]` for each embedding heard, the
/// embedding as its JSON text, to be stored for that person; and
/// `representatives` `[person_ref, voice]` for each person created whose
/// first voice has a norm, its compact text ([`encode_voice`]).
///
/// # Errors
///
/// A static detail when the input's identities are malformed.
pub fn people_for(
    input: &Map<String, Json>,
    known: &[(String, Vec<f64>)],
    after: u64,
) -> Result<Json, &'static str> {
    let mut known = known.to_vec();
    let mut people = Vec::new();
    let mut voices = Vec::new();
    let mut representatives = Vec::new();
    let mut identified = input.clone();
    for name in ["source_ids", "target_ids"] {
        let ids = identities_of(
            input,
            name,
            "conversation:people_for_input input source_ids or target_ids is not an array of identities",
        )?;
        let mut refs: Vec<Json> = Vec::with_capacity(ids.len());
        for id in ids {
            let id = match embedding(id) {
                Some(voice) => {
                    let person = if let Some(person) = person_for_voice(&voice, &known) {
                        person.to_owned()
                    } else {
                        let person = format!(
                            "{PERSON_REF_PREFIX}{}",
                            after.saturating_add(people.len() as u64).saturating_add(1)
                        );
                        people.push(Json::String(person.clone()));
                        if let Some(text) = encode_voice(&voice) {
                            representatives.push(Json::Array(vec![
                                Json::String(person.clone()),
                                Json::String(text),
                            ]));
                        }
                        person
                    };
                    known.push((person.clone(), voice));
                    voices.push(Json::Array(vec![
                        Json::String(person.clone()),
                        Json::String(id.to_string()),
                    ]));
                    Json::String(person)
                }
                None => id.clone(),
            };
            if !refs.contains(&id) {
                refs.push(id);
            }
        }
        identified.insert(name.into(), Json::Array(refs));
    }
    let mut out = Map::new();
    out.insert("input".into(), Json::Object(identified));
    out.insert("people".into(), Json::Array(people));
    out.insert("voices".into(), Json::Array(voices));
    out.insert("representatives".into(), Json::Array(representatives));
    Ok(Json::Object(out))
}

/// A representative folded with more voices: `{centroid, weight,
/// folded}`. The representative (its compact text; one of another form
/// counts for nothing) weighs `weight`, each voice (its JSON text) one:
/// the new representative is the unit vector of `weight` times the
/// representative's unit vector plus each voice's unit vector, a running
/// mean of directions, and its weight `weight` plus the voices folded. A
/// voice of another form, without a norm, or of another dimension than
/// the representative (or the first voice folded) is skipped. With
/// nothing to fold the representative is returned as given.
#[must_use]
pub fn fold_voices(centroid: &str, weight: u64, voices: &[Json]) -> Json {
    let mut sum: Option<Vec<f64>> = decode_voice(centroid).map(|components| {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a person's weight is a count of voices, far under 2^53"
        )]
        let weight = weight as f64;
        unit_vector(&components)
            .iter()
            .map(|component| component * weight)
            .collect()
    });
    let mut weight = if sum.is_some() { weight } else { 0 };
    let mut folded = 0_u64;
    for voice in voices {
        let Some(components) = voice.as_str().and_then(voice_of_text) else {
            continue;
        };
        let unit = unit_vector(&components);
        if encode_voice(&unit).is_none() {
            continue;
        }
        match sum.as_mut() {
            Some(sum) if sum.len() == unit.len() => {
                for (total, component) in sum.iter_mut().zip(&unit) {
                    *total += component;
                }
            }
            Some(_) => continue,
            None => sum = Some(unit),
        }
        weight = weight.saturating_add(1);
        folded += 1;
    }
    let Some(centroid) = sum.as_deref().and_then(encode_voice).filter(|_| folded > 0) else {
        return serde_json::json!({"centroid": centroid, "weight": weight, "folded": 0});
    };
    serde_json::json!({"centroid": centroid, "weight": weight, "folded": folded})
}

/// Executes `<conversation:fold_voices>`.
pub(super) fn fold_voices_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, centroid, weight, voices] = attribute_values(
        attributes,
        ["location", "centroid", "weight", "voices"],
        "conversation:fold_voices takes only the location, centroid, weight and voices attributes",
        "conversation:fold_voices repeats an attribute",
        "conversation:fold_voices has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:fold_voices requires a location",
    ))?;
    let centroid = centroid.ok_or(Failure::Execution(
        "conversation:fold_voices requires a centroid",
    ))?;
    let weight = weight.ok_or(Failure::Execution(
        "conversation:fold_voices requires a weight",
    ))?;
    let voices = voices.ok_or(Failure::Execution(
        "conversation:fold_voices requires voices",
    ))?;
    no_content(content, "conversation:fold_voices holds no content")?;
    let centroid = read_text(
        host,
        centroid,
        [
            "conversation:fold_voices centroid did not evaluate",
            "conversation:fold_voices centroid is not a string",
        ],
    )?;
    let weight = read_whole(
        host,
        weight,
        [
            "conversation:fold_voices weight did not evaluate",
            "conversation:fold_voices weight is not a whole number",
        ],
    )?;
    let voices = read_array(
        host,
        voices,
        [
            "conversation:fold_voices voices did not evaluate",
            "conversation:fold_voices voices is not an array",
        ],
    )?;
    assign_to(
        host,
        location,
        &Data::JsonObject(fold_voices(&centroid, weight, &voices).to_string()),
        "conversation:fold_voices could not assign the representative",
    )?;
    Ok(Executed::Handled)
}

/// The two people to merge first among `people` (`[person_ref, voice,
/// weight]` each, oldest first; a row of another shape is skipped): the
/// pair whose representatives are most alike, at [`SIMILARITY_THRESHOLD`]
/// or more (the first such pair on a tie), as `{into, from, centroid,
/// weight}`: `into` the older of the two, `from` the other, `centroid` the
/// unit vector of their representatives weighted by their weights (each at
/// least one), and `weight` the sum. `Json::Null` when no two are alike
/// enough.
#[must_use]
pub fn people_to_merge(people: &[Json]) -> Json {
    let rows: Vec<(&str, Vec<f64>, u64)> = people
        .iter()
        .filter_map(|row| match row.as_array().map(Vec::as_slice) {
            Some([Json::String(person), Json::String(voice), weight]) if !person.is_empty() => {
                let weight = weight.as_f64().and_then(whole_number)?;
                decode_voice(voice).map(|components| (person.as_str(), components, weight))
            }
            _ => None,
        })
        .collect();
    let mut best: Option<(usize, usize, f64)> = None;
    for (left, (_, older, _)) in rows.iter().enumerate() {
        for (right, (_, newer, _)) in rows.iter().enumerate().skip(left + 1) {
            let Some(score) = cosine_similarity(older, newer) else {
                continue;
            };
            if score >= SIMILARITY_THRESHOLD && best.is_none_or(|(_, _, most)| score > most) {
                best = Some((left, right, score));
            }
        }
    }
    let Some((into, from, _)) = best else {
        return Json::Null;
    };
    let (Some((into, into_voice, into_weight)), Some((from, from_voice, from_weight))) =
        (rows.get(into), rows.get(from))
    else {
        return Json::Null;
    };
    #[allow(
        clippy::cast_precision_loss,
        reason = "a person's weight is a count of voices, far under 2^53"
    )]
    let (left, right) = ((*into_weight).max(1) as f64, (*from_weight).max(1) as f64);
    let into_unit = unit_vector(into_voice);
    let from_unit = unit_vector(from_voice);
    let sum: Vec<f64> = into_unit
        .iter()
        .zip(&from_unit)
        .map(|(older, newer)| older * left + newer * right)
        .collect();
    let Some(centroid) = encode_voice(&sum) else {
        return Json::Null;
    };
    serde_json::json!({
        "into": into,
        "from": from,
        "centroid": centroid,
        "weight": (*into_weight).max(1).saturating_add((*from_weight).max(1)),
    })
}

/// Executes `<conversation:people_to_merge>`.
pub(super) fn people_to_merge_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, people] = attribute_values(
        attributes,
        ["location", "people"],
        "conversation:people_to_merge takes only the location and people attributes",
        "conversation:people_to_merge repeats an attribute",
        "conversation:people_to_merge has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:people_to_merge requires a location",
    ))?;
    let people = people.ok_or(Failure::Execution(
        "conversation:people_to_merge requires people",
    ))?;
    no_content(content, "conversation:people_to_merge holds no content")?;
    let people = read_array(
        host,
        people,
        [
            "conversation:people_to_merge people did not evaluate",
            "conversation:people_to_merge people is not an array",
        ],
    )?;
    let value = match people_to_merge(&people) {
        Json::Null => Data::Null,
        merge => Data::JsonObject(merge.to_string()),
    };
    assign_to(
        host,
        location,
        &value,
        "conversation:people_to_merge could not assign the merge",
    )?;
    Ok(Executed::Handled)
}

/// The relationship context references among `refs` that name the person
/// `from`, each as `[old, new]`: `new` the context reference
/// ([`relationship_context_ref`]) of its identity groups with `from`
/// replaced by `into`, so the merged relationship's memories are kept by
/// the bytes the conversation computes for it. A reference that does not
/// parse as `{"identity_groups": [group, group]}` of identities, or does
/// not change, is left out.
#[must_use]
pub fn merged_context_refs(refs: &[Json], from: &str, into: &str) -> Json {
    let renamed = |group: &[Json]| -> Vec<Json> {
        group
            .iter()
            .map(|id| match id {
                Json::String(name) if name == from => Json::String(into.to_owned()),
                other => other.clone(),
            })
            .collect()
    };
    let pairs: Vec<Json> = refs
        .iter()
        .filter_map(Json::as_str)
        .filter_map(|old| {
            let Ok(Json::Object(parsed)) = serde_json::from_str::<Json>(old) else {
                return None;
            };
            let Some(Json::Array(groups)) = parsed.get("identity_groups") else {
                return None;
            };
            let [Json::Array(left), Json::Array(right)] = groups.as_slice() else {
                return None;
            };
            if parsed.len() != 1 || !left.iter().chain(right).all(is_identity) {
                return None;
            }
            let new = relationship_context_ref(&renamed(left), &renamed(right));
            (new != old).then(|| Json::Array(vec![Json::String(old.to_owned()), Json::String(new)]))
        })
        .collect();
    Json::Array(pairs)
}

/// Executes `<conversation:merged_context_refs>`.
pub(super) fn merged_context_refs_element<H: ConversationHost>(
    attributes: &[Attribute],
    content: &[Content],
    host: &mut H,
) -> Result<Executed, Failure<H::Error>> {
    let [location, refs, from, into] = attribute_values(
        attributes,
        ["location", "refs", "from", "into"],
        "conversation:merged_context_refs takes only the location, refs, from and into attributes",
        "conversation:merged_context_refs repeats an attribute",
        "conversation:merged_context_refs has an empty attribute",
    )?;
    let location = location.ok_or(Failure::Execution(
        "conversation:merged_context_refs requires a location",
    ))?;
    let refs = refs.ok_or(Failure::Execution(
        "conversation:merged_context_refs requires refs",
    ))?;
    let from = from.ok_or(Failure::Execution(
        "conversation:merged_context_refs requires from",
    ))?;
    let into = into.ok_or(Failure::Execution(
        "conversation:merged_context_refs requires into",
    ))?;
    no_content(content, "conversation:merged_context_refs holds no content")?;
    let refs = read_array(
        host,
        refs,
        [
            "conversation:merged_context_refs refs did not evaluate",
            "conversation:merged_context_refs refs is not an array",
        ],
    )?;
    let from = read_text(
        host,
        from,
        [
            "conversation:merged_context_refs from did not evaluate",
            "conversation:merged_context_refs from is not a person",
        ],
    )?;
    let into = read_text(
        host,
        into,
        [
            "conversation:merged_context_refs into did not evaluate",
            "conversation:merged_context_refs into is not a person",
        ],
    )?;
    assign_to(
        host,
        location,
        &Data::JsonArray(merged_context_refs(&refs, &from, &into).to_string()),
        "conversation:merged_context_refs could not assign the references",
    )?;
    Ok(Executed::Handled)
}

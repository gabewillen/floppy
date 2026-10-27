//! A drafted encoding: a reply said while it is still being written.
//!
//! An `encode` without `parts` reads its reply from the events the invoking
//! session sends it (decision D262), once it has returned [`READY_EVENT`]
//! `{}`: [`DRAFT_EVENT`] `{part, words,
//! offset, expression?}`, words a model has newly written into part
//! `part` after `offset` characters (mosfet.py's speaking drafts), and
//! then [`FINAL_EVENT`] `{parts}`, the whole reply. The service says the
//! reply one request at a time, each as soon as it is ready
//! ([`Drafting::next_request`]): the words up to the last sentence that has ended
//! (a `.`, `!`, `?` or `…`, with any closing quotes or brackets, followed
//! by white space), or, past [`MAX_RUN_BYTES`] with no sentence ended, up
//! to the last white space; a part that pauses opens a request of its
//! own, its pause before its words; and, once the whole reply is known,
//! everything left.
//!
//! The whole reply is authoritative: when it continues what was said (each
//! part said is its part, and the part being said begins with what was
//! said of it), the rest of it is said; when it diverges, nothing more is
//! said, and the encoding ends with what was. A draft that does not
//! continue its part (it starts past the part's end) is dropped; one that
//! overlaps it adds only what is new.

use serde_json::Value as Json;

use crate::{MAX_PARTS, MAX_TEXT_BYTES, Pause, Stretch, UNSPOKEN_CHARS, expression_cues};
use scxml_types::exceeds_wasm_bound;

/// What a drafted encoding returns first, once it reads its events, so the
/// invoking session knows where to send them (decision D242's readiness).
pub const READY_EVENT: &str = "bot.ability.ready";

/// Words newly drafted into one part of the reply.
pub const DRAFT_EVENT: &str = "bot.ability.speaking.draft";

/// The whole reply, after its drafts.
pub const FINAL_EVENT: &str = "bot.ability.vocal.speech.encoding.input";

/// The most unsaid bytes held for a sentence to end: past them, the words
/// up to the last white space are said.
pub const MAX_RUN_BYTES: usize = 240;

/// One part of the reply: the pause that opens it and its words so far.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Drafted {
    pause: Option<Pause>,
    words: String,
}

/// The reply as drafted so far, and how much of it is said.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Drafting {
    parts: Vec<Drafted>,
    /// The next part to say from, and the bytes of it already said.
    part: usize,
    byte: usize,
    /// Whether the whole reply arrived.
    settled: bool,
    /// Whether it diverged from what was said.
    diverged: bool,
    /// The bytes of words drafted, bounded by [`MAX_TEXT_BYTES`].
    bytes: usize,
}

impl Drafting {
    /// Takes one [`DRAFT_EVENT`]'s data; a draft that is malformed, past
    /// [`MAX_PARTS`] or the text bound, after the whole reply, or that does
    /// not continue its part is dropped.
    pub fn draft(&mut self, data: &Json) {
        if self.settled {
            return;
        }
        let (Some(part), Some(words), Some(offset)) = (
            data.get("part")
                .and_then(Json::as_u64)
                .and_then(|part| usize::try_from(part).ok()),
            data.get("words").and_then(Json::as_str),
            data.get("offset")
                .and_then(Json::as_u64)
                .and_then(|offset| usize::try_from(offset).ok()),
        ) else {
            return;
        };
        if part >= MAX_PARTS || part < self.part {
            return;
        }
        if self.parts.len() <= part {
            self.parts.resize_with(part + 1, Drafted::default);
        }
        let Some(drafted) = self.parts.get_mut(part) else {
            return;
        };
        let written = drafted.words.chars().count();
        if offset == 0
            && written == 0
            && let Some(expression) = data.get("expression")
            && let Ok((pause, _)) = expression_cues(expression)
        {
            drafted.pause = pause;
        }
        let Some(overlap) = written.checked_sub(offset) else {
            return;
        };
        let new: String = words.chars().skip(overlap).collect();
        if exceeds_wasm_bound(self.bytes.saturating_add(new.len()), MAX_TEXT_BYTES) {
            return;
        }
        self.bytes += new.len();
        drafted.words.push_str(&new);
    }

    /// Takes [`FINAL_EVENT`]'s `parts` (already checked as an encoding's
    /// parts): the rest of the reply when it continues what was said,
    /// nothing more when it diverges.
    pub fn settle(&mut self, parts: &[Json]) {
        if self.settled {
            return;
        }
        self.settled = true;
        let whole: Vec<Drafted> = parts
            .iter()
            .map(|part| Drafted {
                pause: part
                    .get("expression")
                    .and_then(|expression| expression_cues(expression).ok())
                    .and_then(|(pause, _)| pause),
                words: part
                    .get("words")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect();
        let said_whole = self.parts.get(..self.part).is_some_and(|said| {
            whole
                .get(..self.part)
                .is_some_and(|whole_said| whole_said == said)
        });
        let saying = match (self.parts.get(self.part), whole.get(self.part)) {
            (Some(drafted), Some(whole)) => {
                (self.byte == 0 || drafted.pause == whole.pause)
                    && drafted
                        .words
                        .get(..self.byte)
                        .is_some_and(|said| whole.words.starts_with(said))
            }
            (None, _) => true,
            (Some(_), None) => self.byte == 0,
        };
        if said_whole && saying {
            self.parts = whole;
        } else {
            self.diverged = true;
        }
    }

    /// Whether the whole reply arrived and all of it that will be said
    /// was.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.settled && (self.diverged || self.part >= self.parts.len())
    }

    /// The next request to make, when one is ready: the pause before it and
    /// its words (blank only for a pause that ends the reply).
    pub fn next_request(&mut self) -> Option<Stretch> {
        if self.diverged {
            return None;
        }
        loop {
            let first = self.parts.get(self.part)?;
            // A part's pause is said with its first words.
            let pause = if self.byte > 0 { None } else { first.pause };
            // The words from here to the next part that pauses.
            let mut words = first.words.get(self.byte..).unwrap_or_default().to_owned();
            let mut last = self.part;
            for (index, part) in self.parts.iter().enumerate().skip(self.part + 1) {
                if part.pause.is_some() {
                    break;
                }
                words.push_str(&part.words);
                last = index;
            }
            // Ended by a later part that pauses, or by the whole reply.
            let closed = self.settled || last + 1 < self.parts.len();
            let take = if closed { words.len() } else { ready(&words) };
            if take == 0 {
                if closed {
                    self.part = last + 1;
                    self.byte = 0;
                    if pause.is_some() {
                        return Some(Stretch {
                            pause,
                            words: String::new(),
                        });
                    }
                    continue;
                }
                return None;
            }
            let said = words.get(..take).unwrap_or_default().to_owned();
            self.advance(take);
            return Some(Stretch {
                pause,
                words: said.replace(UNSPOKEN_CHARS, ""),
            });
        }
    }

    /// Moves what is said on by `take` bytes of the words from here.
    fn advance(&mut self, mut take: usize) {
        while let Some(part) = self.parts.get(self.part) {
            let left = part.words.len().saturating_sub(self.byte);
            if take < left || (take == left && self.part + 1 >= self.parts.len()) {
                self.byte += take;
                return;
            }
            take -= left;
            self.part += 1;
            self.byte = 0;
            if take == 0 {
                return;
            }
        }
    }
}

/// The bytes of `words` ready to say before the reply is whole: through
/// the last sentence that ended, or, past [`MAX_RUN_BYTES`] with none, to
/// the last white space; 0 when none is ready.
fn ready(words: &str) -> usize {
    let mut ended = 0;
    let mut space = 0;
    let mut terminated = false;
    for (at, character) in words.char_indices() {
        if character.is_whitespace() {
            let after = at + character.len_utf8();
            if terminated {
                ended = after;
            }
            space = after;
            terminated = false;
        } else if matches!(character, '.' | '!' | '?' | '…') {
            terminated = true;
        } else if !matches!(character, '"' | '\'' | ')' | ']' | '”' | '’' | '»') {
            terminated = false;
        }
    }
    if ended == 0 && words.len() > MAX_RUN_BYTES {
        space
    } else {
        ended
    }
}

#[cfg(test)]
mod tests {
    use super::{Drafting, MAX_RUN_BYTES, ready};
    use crate::{Pause, Stretch};
    use serde_json::json;

    fn said(drafting: &mut Drafting) -> Vec<Stretch> {
        std::iter::from_fn(|| drafting.next_request()).collect()
    }

    fn words(text: &str) -> Stretch {
        Stretch {
            pause: None,
            words: text.into(),
        }
    }

    #[test]
    fn a_sentence_is_ready_once_white_space_follows_its_end() {
        assert_eq!(ready("Hello there"), 0);
        assert_eq!(ready("Hello there."), 0);
        assert_eq!(ready("Hello there. How"), "Hello there. ".len());
        assert_eq!(ready("Wow!\" Then? Yes"), "Wow!\" Then? ".len());
        assert_eq!(ready("3.5 is a number"), 0);
        let run = "word ".repeat(MAX_RUN_BYTES / 5 + 1);
        assert_eq!(ready(&format!("{run}x")), run.len());
    }

    #[test]
    fn each_sentence_is_said_as_it_ends_and_the_rest_once_the_reply_is_whole() {
        let mut drafting = Drafting::default();
        drafting.draft(&json!({"part": 0, "words": "Hi there. How", "offset": 0}));
        assert_eq!(said(&mut drafting), [words("Hi there. ")]);
        drafting.draft(&json!({"part": 0, "words": " are you? I am", "offset": 13}));
        assert_eq!(said(&mut drafting), [words("How are you? ")]);
        drafting.draft(&json!({"part": 0, "words": " fine", "offset": 27}));
        assert!(said(&mut drafting).is_empty());
        assert!(!drafting.finished());
        drafting.settle(&[json!({"words": "Hi there. How are you? I am fine."})]);
        assert_eq!(said(&mut drafting), [words("I am fine.")]);
        assert!(drafting.finished());
    }

    #[test]
    fn a_part_that_pauses_opens_its_own_request() {
        let mut drafting = Drafting::default();
        drafting.draft(&json!({"part": 0, "words": "Well", "offset": 0}));
        drafting
            .draft(&json!({"part": 1, "words": "", "offset": 0, "expression": {"pause": "short"}}));
        drafting.draft(&json!({"part": 1, "words": "so. And", "offset": 0}));
        assert_eq!(
            said(&mut drafting),
            [
                words("Well"),
                Stretch {
                    pause: Some(Pause::Short),
                    words: "so. ".into()
                }
            ]
        );
        drafting.settle(&[
            json!({"words": "Well"}),
            json!({"expression": {"pause": "short"}, "words": "so. And then."}),
            json!({"expression": {"pause": "long"}}),
        ]);
        assert_eq!(
            said(&mut drafting),
            [
                words("And then."),
                Stretch {
                    pause: Some(Pause::Long),
                    words: String::new()
                }
            ]
        );
        assert!(drafting.finished());
    }

    #[test]
    fn overlapping_drafts_add_what_is_new_and_gaps_are_dropped() {
        let mut drafting = Drafting::default();
        drafting.draft(&json!({"part": 0, "words": "One", "offset": 0}));
        drafting.draft(&json!({"part": 0, "words": "ne. Two", "offset": 1}));
        drafting.draft(&json!({"part": 0, "words": "lost", "offset": 40}));
        assert_eq!(said(&mut drafting), [words("One. ")]);
        drafting.settle(&[json!({"words": "One. Two."})]);
        assert_eq!(said(&mut drafting), [words("Two.")]);
    }

    #[test]
    fn a_whole_reply_that_diverges_from_what_was_said_says_nothing_more() {
        let mut drafting = Drafting::default();
        drafting.draft(&json!({"part": 0, "words": "Yes. And", "offset": 0}));
        assert_eq!(said(&mut drafting), [words("Yes. ")]);
        drafting.settle(&[json!({"words": "No. Never."})]);
        assert!(said(&mut drafting).is_empty());
        assert!(drafting.finished());
    }

    #[test]
    fn a_reply_with_no_drafts_is_said_whole_when_it_arrives() {
        let mut drafting = Drafting::default();
        assert!(said(&mut drafting).is_empty());
        drafting.settle(&[json!({"words": "All "}), json!({"words": "at once."})]);
        assert_eq!(said(&mut drafting), [words("All at once.")]);
        assert!(drafting.finished());
    }
}

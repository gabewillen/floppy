//! Static SCXML 1.0 element and attribute vocabulary used to explain XSD
//! findings with concrete repairs.
//!
//! The table restates the official W3C schema bundled in
//! `schemas/w3c-scxml-1.0/` (element content models, attribute names,
//! required attributes, enumerations, and fixed values) together with the
//! SCXML 1.0 Recommendation section of each element. It is used only for
//! presentation — typo suggestions, allowed-value lists, allowed-children
//! lists, and spec citations — and never decides validity: every finding is
//! produced by the XSD backend. A unit test checks the table against the
//! bundled schema so the two cannot drift silently.

/// Executable content children accepted by `onentry`, `onexit`,
/// `transition`, `if`, `elseif`, `else`, `foreach`, and `finalize`.
pub const EXECUTABLE_CONTENT: &[&str] = &[
    "raise", "if", "foreach", "send", "script", "assign", "log", "cancel",
];

/// One attribute declared for an SCXML element.
#[derive(Clone, Copy, Debug)]
pub struct AttributeSpec {
    /// Unqualified attribute name.
    pub name: &'static str,
    /// Whether the schema declares `use="required"`.
    pub required: bool,
    /// Exact enumeration or fixed value, when the datatype restricts values.
    pub values: &'static [&'static str],
    /// Human-readable lexical form for placeholders and help.
    pub lexical: &'static str,
}

/// One SCXML element with its schema-declared shape.
#[derive(Clone, Copy, Debug)]
pub struct ElementSpec {
    /// Local name in the SCXML namespace.
    pub name: &'static str,
    /// Declared attributes.
    pub attributes: &'static [AttributeSpec],
    /// Allowed SCXML child elements (elements from other namespaces are
    /// always allowed by `scxml.extra.content`).
    pub children: &'static [&'static str],
    /// Whether the content model accepts executable content children.
    pub executable_children: bool,
    /// Whether character content is meaningful inside the element.
    pub mixed: bool,
    /// SCXML 1.0 Recommendation section citation.
    pub spec: &'static str,
}

const fn attribute(name: &'static str, lexical: &'static str) -> AttributeSpec {
    AttributeSpec {
        name,
        required: false,
        values: &[],
        lexical,
    }
}

const fn required(name: &'static str, lexical: &'static str) -> AttributeSpec {
    AttributeSpec {
        name,
        required: true,
        values: &[],
        lexical,
    }
}

const fn enumerated(name: &'static str, values: &'static [&'static str]) -> AttributeSpec {
    AttributeSpec {
        name,
        required: false,
        values,
        lexical: "one of the listed values",
    }
}

const EXPR: &str = "a value expression in the selected data model";
const COND: &str = "a boolean condition expression in the selected data model";
const LOCATION: &str = "a data-model location expression";
const IDREFS: &str = "one or more whitespace-separated state IDs";
const ID: &str = "an XML NCName unique within the document";
const URI: &str = "a URI";
const DURATION: &str = "a CSS2 duration such as `500ms`, `2s`, or `1.5m`";
const EVENT: &str = "an event name such as `done` or `error.execution`";
const EVENTS: &str = "whitespace-separated event descriptors such as `a b.c` or `*`";

/// Complete SCXML 1.0 vocabulary as declared by the bundled W3C schema.
pub const ELEMENTS: &[ElementSpec] = &[
    ElementSpec {
        name: "scxml",
        attributes: &[
            attribute("initial", IDREFS),
            attribute("name", "an XML NMTOKEN"),
            AttributeSpec {
                name: "version",
                required: true,
                values: &["1.0"],
                lexical: "the fixed value `1.0`",
            },
            attribute(
                "datamodel",
                "a data model name such as `ecmascript`, `xpath`, or `null`",
            ),
            enumerated("binding", &["early", "late"]),
            enumerated("exmode", &["lax", "strict"]),
        ],
        children: &["state", "parallel", "final", "datamodel", "script"],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §3.2 <scxml>",
    },
    ElementSpec {
        name: "state",
        attributes: &[attribute("id", ID), attribute("initial", IDREFS)],
        children: &[
            "onentry",
            "onexit",
            "transition",
            "initial",
            "state",
            "parallel",
            "final",
            "history",
            "datamodel",
            "invoke",
        ],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §3.3 <state>",
    },
    ElementSpec {
        name: "parallel",
        attributes: &[attribute("id", ID)],
        children: &[
            "onentry",
            "onexit",
            "transition",
            "state",
            "parallel",
            "history",
            "datamodel",
            "invoke",
        ],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §3.4 <parallel>",
    },
    ElementSpec {
        name: "transition",
        attributes: &[
            attribute("event", EVENTS),
            attribute("cond", COND),
            attribute("target", IDREFS),
            enumerated("type", &["internal", "external"]),
        ],
        children: &[],
        executable_children: true,
        mixed: false,
        spec: "SCXML 1.0 §3.5 <transition>",
    },
    ElementSpec {
        name: "initial",
        attributes: &[],
        children: &["transition"],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §3.6 <initial>",
    },
    ElementSpec {
        name: "final",
        attributes: &[attribute("id", ID)],
        children: &["onentry", "onexit", "donedata"],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §3.7 <final>",
    },
    ElementSpec {
        name: "onentry",
        attributes: &[],
        children: &[],
        executable_children: true,
        mixed: false,
        spec: "SCXML 1.0 §3.8 <onentry>",
    },
    ElementSpec {
        name: "onexit",
        attributes: &[],
        children: &[],
        executable_children: true,
        mixed: false,
        spec: "SCXML 1.0 §3.9 <onexit>",
    },
    ElementSpec {
        name: "history",
        attributes: &[
            attribute("id", ID),
            enumerated("type", &["shallow", "deep"]),
        ],
        children: &["transition"],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §3.10 <history>",
    },
    ElementSpec {
        name: "raise",
        attributes: &[required("event", EVENT)],
        children: &[],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §4.2 <raise>",
    },
    ElementSpec {
        name: "if",
        attributes: &[required("cond", COND)],
        children: &["elseif", "else"],
        executable_children: true,
        mixed: false,
        spec: "SCXML 1.0 §4.3 <if>",
    },
    ElementSpec {
        name: "elseif",
        attributes: &[required("cond", COND)],
        children: &[],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §4.4 <elseif>",
    },
    ElementSpec {
        name: "else",
        attributes: &[],
        children: &[],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §4.5 <else>",
    },
    ElementSpec {
        name: "foreach",
        attributes: &[
            required("array", EXPR),
            required("item", "a variable name"),
            attribute("index", "a variable name"),
        ],
        children: &[],
        executable_children: true,
        mixed: false,
        spec: "SCXML 1.0 §4.6 <foreach>",
    },
    ElementSpec {
        name: "log",
        attributes: &[attribute("label", "a string"), attribute("expr", EXPR)],
        children: &[],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §4.7 <log>",
    },
    ElementSpec {
        name: "datamodel",
        attributes: &[],
        children: &["data"],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §5.2 <datamodel>",
    },
    ElementSpec {
        name: "data",
        attributes: &[
            required("id", ID),
            attribute("src", URI),
            attribute("expr", EXPR),
        ],
        children: &[],
        executable_children: false,
        mixed: true,
        spec: "SCXML 1.0 §5.3 <data>",
    },
    ElementSpec {
        name: "assign",
        attributes: &[
            required("location", LOCATION),
            attribute("expr", EXPR),
            enumerated(
                "type",
                &[
                    "replacechildren",
                    "firstchild",
                    "lastchild",
                    "previoussibling",
                    "nextsibling",
                    "replace",
                    "delete",
                    "addattribute",
                ],
            ),
            attribute("attr", "an XML NMTOKEN"),
        ],
        children: &[],
        executable_children: false,
        mixed: true,
        spec: "SCXML 1.0 §5.4 <assign>",
    },
    ElementSpec {
        name: "donedata",
        attributes: &[],
        children: &["content", "param"],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §5.5 <donedata>",
    },
    ElementSpec {
        name: "content",
        attributes: &[attribute("expr", EXPR)],
        children: &[],
        executable_children: false,
        mixed: true,
        spec: "SCXML 1.0 §5.6 <content>",
    },
    ElementSpec {
        name: "param",
        attributes: &[
            required("name", "an XML NMTOKEN"),
            attribute("expr", EXPR),
            attribute("location", LOCATION),
        ],
        children: &[],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §5.7 <param>",
    },
    ElementSpec {
        name: "script",
        attributes: &[attribute("src", URI)],
        children: &[],
        executable_children: false,
        mixed: true,
        spec: "SCXML 1.0 §5.8 <script>",
    },
    ElementSpec {
        name: "send",
        attributes: &[
            attribute("event", EVENT),
            attribute("eventexpr", EXPR),
            attribute("target", URI),
            attribute("targetexpr", EXPR),
            attribute("type", URI),
            attribute("typeexpr", EXPR),
            attribute("id", ID),
            attribute("idlocation", LOCATION),
            attribute("delay", DURATION),
            attribute("delayexpr", EXPR),
            attribute("namelist", "whitespace-separated data-model locations"),
        ],
        children: &["content", "param"],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §6.2 <send>",
    },
    ElementSpec {
        name: "cancel",
        attributes: &[
            attribute("sendid", "the `id` of a `<send>`"),
            attribute("sendidexpr", EXPR),
        ],
        children: &[],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §6.3 <cancel>",
    },
    ElementSpec {
        name: "invoke",
        attributes: &[
            attribute("type", URI),
            attribute("typeexpr", EXPR),
            attribute("src", URI),
            attribute("srcexpr", EXPR),
            attribute("id", ID),
            attribute("idlocation", LOCATION),
            attribute("namelist", "whitespace-separated data-model locations"),
            enumerated("autoforward", &["true", "false"]),
        ],
        children: &["content", "param", "finalize"],
        executable_children: false,
        mixed: false,
        spec: "SCXML 1.0 §6.4 <invoke>",
    },
    ElementSpec {
        name: "finalize",
        attributes: &[],
        children: &[],
        executable_children: true,
        mixed: false,
        spec: "SCXML 1.0 §6.5 <finalize>",
    },
];

/// Returns the SCXML element named `local_name`.
#[must_use]
pub fn element(local_name: &str) -> Option<&'static ElementSpec> {
    ELEMENTS.iter().find(|spec| spec.name == local_name)
}

impl ElementSpec {
    /// Returns the declared attribute named `name`.
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&'static AttributeSpec> {
        self.attributes.iter().find(|spec| spec.name == name)
    }

    /// Every SCXML child element name the content model accepts, in schema order.
    #[must_use]
    pub fn allowed_children(&self) -> Vec<&'static str> {
        let mut children = self.children.to_vec();
        if self.executable_children {
            children.extend_from_slice(EXECUTABLE_CONTENT);
        }
        children
    }
}

/// Returns the nearest candidate within a small edit distance, deterministic
/// on ties (first in iteration order wins). Inputs are bounded to 64 chars.
#[must_use]
pub fn nearest<'a>(needle: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    closest(needle, candidates).first().copied()
}

/// Every candidate at the smallest edit distance from `needle` within the
/// length-scaled threshold (1 edit up to 3 characters, 2 up to 7, otherwise
/// 3), in iteration order without repeats; empty when none is that close.
/// More than one entry means the misspelling is ambiguous. Inputs are
/// bounded to 64 chars.
#[must_use]
pub fn closest<'a>(needle: &str, candidates: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let needle: Vec<char> = needle.chars().take(65).collect();
    if needle.len() > 64 || needle.is_empty() {
        return Vec::new();
    }
    let threshold = match needle.len() {
        0..=3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    let mut best = threshold;
    let mut found: Vec<&str> = Vec::new();
    for candidate in candidates {
        let chars: Vec<char> = candidate.chars().take(65).collect();
        if chars.len() > 64 {
            continue;
        }
        let distance = edit_distance(&needle, &chars);
        if distance == 0 || distance > best {
            continue;
        }
        if distance < best {
            best = distance;
            found.clear();
        }
        if !found.contains(&candidate) {
            found.push(candidate);
        }
    }
    found
}

/// An attribute written on an SCXML element whose schema does not declare
/// it, while the schema declares it on `owner` for the related role and
/// declares `rename` on `element` for this element's side of that role.
/// Each entry restates the bundled W3C schema and the cited Recommendation
/// sections; a unit test checks every name against [`ELEMENTS`].
#[derive(Clone, Copy, Debug)]
pub struct Counterpart {
    /// The element the attribute was written on.
    pub element: &'static str,
    /// The undeclared attribute.
    pub attribute: &'static str,
    /// An element whose schema declares `attribute`.
    pub owner: &'static str,
    /// The attribute `element` declares for the same role.
    pub rename: &'static str,
    /// Whether the value keeps its meaning under `rename`, so renaming is
    /// the whole repair (machine-applicable); otherwise it is a guess
    /// (maybe-incorrect).
    pub same_value: bool,
    /// Why `rename` is this element's attribute, for help.
    pub reason: &'static str,
}

/// Attributes of one SCXML element that authors write on a related one.
pub const COUNTERPARTS: &[Counterpart] = &[
    // SCXML 1.0 §6.2.1: `id` is "the ID of the send"; §6.3.1: `sendid` is
    // "The ID of the event which is to be cancelled".
    Counterpart {
        element: "send",
        attribute: "sendid",
        owner: "cancel",
        rename: "id",
        same_value: true,
        reason: "a `<send>` declares its ID with `id`; `sendid` is how `<cancel>` refers to that ID",
    },
    Counterpart {
        element: "cancel",
        attribute: "id",
        owner: "send",
        rename: "sendid",
        same_value: true,
        reason: "`<cancel>` names the `id` of the `<send>` to cancel with `sendid`",
    },
    // SCXML 1.0 §3.5.1, §4.3.1, §4.4.1: the guard is `cond`, "a boolean
    // expression"; `expr` is a value expression (§5.4 `<assign>`, §4.7 `<log>`).
    Counterpart {
        element: "transition",
        attribute: "expr",
        owner: "assign",
        rename: "cond",
        same_value: true,
        reason: "a `<transition>` is guarded by the boolean expression `cond`; `expr` is a value expression of `<assign>`, `<log>`, `<data>`, `<param>`, and `<content>`",
    },
    Counterpart {
        element: "if",
        attribute: "expr",
        owner: "assign",
        rename: "cond",
        same_value: true,
        reason: "an `<if>` tests the boolean expression `cond`; `expr` is a value expression of `<assign>`, `<log>`, `<data>`, `<param>`, and `<content>`",
    },
    Counterpart {
        element: "elseif",
        attribute: "expr",
        owner: "assign",
        rename: "cond",
        same_value: true,
        reason: "an `<elseif>` tests the boolean expression `cond`; `expr` is a value expression of `<assign>`, `<log>`, `<data>`, `<param>`, and `<content>`",
    },
    // SCXML 1.0 §3.3.1: `initial` is "The id of the default initial state
    // (or states) for this state"; §3.5.1: `target` belongs to `<transition>`,
    // the only way to leave a state. Which one the author meant is a guess.
    Counterpart {
        element: "state",
        attribute: "target",
        owner: "transition",
        rename: "initial",
        same_value: false,
        reason: "a `<state>` leaves through a `<transition target=\"…\">` child; its own `initial` names the child state entered first",
    },
];

/// The counterpart of `attribute` written on `element`, when one is known.
#[must_use]
pub fn counterpart(element: &str, attribute: &str) -> Option<&'static Counterpart> {
    COUNTERPARTS
        .iter()
        .find(|entry| entry.element == element && entry.attribute == attribute)
}

/// Optimal-string-alignment distance: insertions, deletions, substitutions,
/// and adjacent transpositions (`lgo` -> `log`) each cost one edit.
fn edit_distance(left: &[char], right: &[char]) -> usize {
    let width = right.len() + 1;
    let mut rows = vec![vec![0_usize; width]; 3];
    let cell = |rows: &Vec<Vec<usize>>, row: usize, column: usize| -> usize {
        rows.get(row % 3)
            .and_then(|values| values.get(column))
            .copied()
            .unwrap_or(usize::MAX / 2)
    };
    if let Some(first) = rows.first_mut() {
        for (column, value) in first.iter_mut().enumerate() {
            *value = column;
        }
    }
    for row in 1..=left.len() {
        for column in 0..width {
            let value = if column == 0 {
                row
            } else {
                let left_character = left.get(row - 1);
                let right_character = right.get(column - 1);
                let substitution = usize::from(left_character != right_character);
                let mut value = (cell(&rows, row - 1, column) + 1)
                    .min(cell(&rows, row, column - 1) + 1)
                    .min(cell(&rows, row - 1, column - 1) + substitution);
                if row > 1
                    && column > 1
                    && left_character == right.get(column - 2)
                    && left.get(row - 2) == right_character
                {
                    value = value.min(cell(&rows, row - 2, column - 2) + 1);
                }
                value
            };
            if let Some(slot) = rows
                .get_mut(row % 3)
                .and_then(|values| values.get_mut(column))
            {
                *slot = value;
            }
        }
    }
    cell(&rows, left.len(), right.len())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::bundled::W3C_SCXML_SCHEMA_DOCUMENTS;

    /// Every element, attribute, and requiredness in the table must be
    /// declared by the bundled W3C schema text.
    #[test]
    fn vocabulary_matches_bundled_w3c_schema_declarations() {
        let schema: String = W3C_SCXML_SCHEMA_DOCUMENTS
            .iter()
            .map(|document| String::from_utf8_lossy(document.bytes).into_owned())
            .collect();
        let mut names = BTreeSet::new();
        for element in ELEMENTS {
            assert!(names.insert(element.name), "duplicate {}", element.name);
            assert!(
                schema.contains(&format!("<xsd:element name=\"{}\"", element.name)),
                "schema does not declare element {}",
                element.name
            );
            for attribute in element.attributes {
                let declared = format!("<xsd:attribute name=\"{}\"", attribute.name);
                assert!(
                    schema.contains(&declared),
                    "schema does not declare attribute {}",
                    attribute.name
                );
            }
            for child in element.allowed_children() {
                assert!(
                    schema.contains(&format!("<xsd:element ref=\"{child}\"")),
                    "schema never references child {child}"
                );
            }
        }
        for declared in schema.split("<xsd:element name=\"").skip(1) {
            let name = declared.split('"').next().unwrap_or_default();
            assert!(names.contains(name), "table is missing element {name}");
        }
        for (element, attribute) in [
            ("scxml", "version"),
            ("if", "cond"),
            ("elseif", "cond"),
            ("foreach", "array"),
            ("foreach", "item"),
            ("raise", "event"),
            ("data", "id"),
            ("param", "name"),
            ("assign", "location"),
        ] {
            assert!(
                element_attribute(element, attribute).is_some_and(|spec| spec.required),
                "{element}/@{attribute} must be required"
            );
        }
        assert_eq!(schema.matches("use=\"required\"").count(), 9);
    }

    fn element_attribute(element_name: &str, name: &str) -> Option<&'static AttributeSpec> {
        element(element_name).and_then(|spec| spec.attribute(name))
    }

    #[test]
    fn nearest_is_bounded_and_deterministic() {
        assert_eq!(nearest("evnt", ["event", "cond", "target"]), Some("event"));
        assert_eq!(
            nearest("trasition", ["transition", "state"]),
            Some("transition")
        );
        assert_eq!(nearest("zzzzzz", ["event"]), None);
        assert_eq!(
            nearest("lgo", ["raise", "log", "if"]),
            Some("log"),
            "a transposition is one edit"
        );
        assert_eq!(nearest(&"x".repeat(80), ["x"]), None);
    }

    #[test]
    fn closest_lists_every_tie() {
        assert_eq!(closest("evnt", ["event", "cond", "target"]), ["event"]);
        let send =
            element("send").map(|spec| spec.attributes.iter().map(|attribute| attribute.name));
        assert_eq!(
            closest("tage", send.into_iter().flatten()),
            ["target", "type"],
            "two edits from both"
        );
        assert!(closest("zzzzzz", ["event"]).is_empty());
    }

    /// Every counterpart restates the bundled schema: the attribute is
    /// undeclared on its element and declared on the owner, and the rename
    /// is declared on the element.
    #[test]
    fn counterparts_match_the_schema_vocabulary() {
        for entry in COUNTERPARTS {
            let spec = element(entry.element).expect("the element is SCXML");
            assert!(spec.attribute(entry.attribute).is_none(), "{entry:?}");
            assert!(spec.attribute(entry.rename).is_some(), "{entry:?}");
            assert!(
                element_attribute(entry.owner, entry.attribute).is_some(),
                "{entry:?}"
            );
            assert!(
                closest(
                    entry.attribute,
                    spec.attributes.iter().map(|attribute| attribute.name)
                )
                .is_empty(),
                "a typo suggestion would compete with {entry:?}"
            );
        }
    }
}

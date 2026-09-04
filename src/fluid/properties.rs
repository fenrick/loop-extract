//! The `props` map carried by merge-tree segments.
//!
//! Observed in test fixtures:
//! Property keys use a `namespace!name` convention (`format!bold`,
//! `list!reference`, `style!reference`). Keys without a `!` are structural
//! (`markerId`, `nodeType`, `attribution`).
//!
//! Every property is preserved in the internal model. Only properties whose
//! behaviour is demonstrated by the corpus are given typed accessors; the rest
//! stay available as raw JSON for diagnostics.
//!
//! This is reverse-engineered behaviour and is not based on a published
//! Microsoft Prague/Fluid file-format specification.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROP_BOLD: &str = "format!bold";
pub const PROP_ITALIC: &str = "format!italic";
pub const PROP_STYLE_REFERENCE: &str = "style!reference";
pub const PROP_LIST_REFERENCE: &str = "list!reference";
pub const PROP_HYPERLINK_URL: &str = "hyperlink!url";
pub const PROP_PLACEHOLDER_ATTRIBUTE_TEXT: &str = "placeholder!attributeText";
pub const PROP_NODE_TYPE: &str = "nodeType";
pub const PROP_MARKER_ID: &str = "markerId";
pub const PROP_ATTRIBUTION: &str = "attribution";

/// Value seen on the marker that terminates the document title.
pub const PLACEHOLDER_ADD_TITLE: &str = "placeholder!addTitle";

/// How well a property key is understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PropertyClass {
    /// Understood and used to reconstruct the document.
    Rendered,
    /// Recognised in the corpus but deliberately not rendered.
    Recognised,
    /// Not seen before. Preserved and reported.
    Unknown,
}

/// Classifies a property key. Prefix matching covers the indexed
/// `list!itemFormat-0` .. `list!itemFormat-8` family and the per-list
/// `list!list-<guid><n>` keys, whose exact semantics are not yet established.
pub fn classify(key: &str) -> PropertyClass {
    match key {
        PROP_BOLD | PROP_ITALIC | PROP_STYLE_REFERENCE | PROP_LIST_REFERENCE
        | PROP_HYPERLINK_URL | PROP_NODE_TYPE => PropertyClass::Rendered,

        PROP_MARKER_ID
        | PROP_ATTRIBUTION
        | PROP_PLACEHOLDER_ATTRIBUTE_TEXT
        | "list!id"
        | "paragraph!textAlignment"
        | "paragraph!writingMode"
        | "proofing!spellingError"
        | "proofing!grammarError"
        | "content!locale"
        | "content!detectedLocale"
        | "component!display"
        | "component!height"
        | "component!width"
        | "component!mimeType"
        | "component!routerInput"
        | "component!url"
        | "component!urlTitle"
        | "tab!indentCountLeft"
        | "tab!indentCountRight"
        | "aria!role" => PropertyClass::Recognised,

        _ if key.starts_with("list!itemFormat-") || key.starts_with("list!list-") => {
            PropertyClass::Recognised
        }
        _ if is_property_attribution(key) => PropertyClass::Recognised,
        _ => PropertyClass::Unknown,
    }
}

/// True for a key that records *who set another property*.
///
/// Observed in test fixtures: one page carries
/// `"@hyperlink!url": {"id": "<address>", "name": "<name>", "timestamp": ...}`,
/// alongside the ordinary `hyperlink!url`. The value is an attribution record,
/// not a hyperlink.
///
/// ASSUMPTION, generalised from that single observed key: a leading `@` marks
/// per-property attribution for the property named after it. `@hyperlink!url`
/// is the only such key in the corpus (36 occurrences, one file), so the
/// generalisation is unproven. It is safe either way: the only behaviour that
/// depends on it is treating the value as personal data.
pub fn is_property_attribution(key: &str) -> bool {
    key.starts_with('@') && key.len() > 1
}

/// True for a property whose value identifies a person.
///
/// Renderers must omit these unless attribution was explicitly requested. The
/// parser always keeps them: filtering belongs to the output layer, so later
/// analysis stays possible without exposing personal information by default.
pub fn is_personal_data(key: &str) -> bool {
    key == PROP_ATTRIBUTION || is_property_attribution(key)
}

/// A segment's property map, preserved whole.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Properties(pub Map<String, Value>);

impl Properties {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    fn bool_prop(&self, key: &str) -> Option<bool> {
        self.0.get(key)?.as_bool()
    }

    fn str_prop(&self, key: &str) -> Option<&str> {
        self.0.get(key)?.as_str()
    }

    pub fn bold(&self) -> Option<bool> {
        self.bool_prop(PROP_BOLD)
    }

    pub fn italic(&self) -> Option<bool> {
        self.bool_prop(PROP_ITALIC)
    }

    /// Observed values include `"Heading 1"`.
    pub fn style_reference(&self) -> Option<&str> {
        self.str_prop(PROP_STYLE_REFERENCE)
    }

    pub fn node_type(&self) -> Option<&str> {
        self.str_prop(PROP_NODE_TYPE)
    }

    pub fn marker_id(&self) -> Option<&str> {
        self.str_prop(PROP_MARKER_ID)
    }

    pub fn hyperlink_url(&self) -> Option<&str> {
        self.str_prop(PROP_HYPERLINK_URL)
    }

    pub fn placeholder_attribute_text(&self) -> Option<&str> {
        self.str_prop(PROP_PLACEHOLDER_ATTRIBUTE_TEXT)
    }

    /// True when this marker terminates the document title.
    pub fn is_add_title_placeholder(&self) -> bool {
        self.placeholder_attribute_text() == Some(PLACEHOLDER_ADD_TITLE)
    }

    pub fn attribution(&self) -> Option<&Value> {
        self.0.get(PROP_ATTRIBUTION)
    }

    pub fn list_reference(&self) -> Option<ListReference> {
        ListReference::parse(self.str_prop(PROP_LIST_REFERENCE)?)
    }

    /// Property keys grouped by how well they are understood.
    pub fn classified_keys(&self) -> impl Iterator<Item = (&str, PropertyClass)> {
        self.0.keys().map(|k| (k.as_str(), classify(k)))
    }
}

/// A parsed `list!reference` value.
///
/// Observed in test fixtures: values take the form `"<n>;list!list-<guid><m>"`,
/// for example `"1;list!list-<guid>0"`.
///
/// ASSUMPTION, not yet confirmed: the leading integer is the nesting depth and
/// the remainder identifies the list. The trailing digit on the list id is not
/// interpreted, because two ids differing only in that digit occur in the same
/// document and their relationship is unestablished. Nothing downstream may
/// depend on the depth reading until a test demonstrates it against known
/// nesting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListReference {
    pub raw: String,
    /// The leading integer, if the value has the observed shape.
    pub depth: Option<u32>,
    /// Everything after the first `;`.
    pub list_id: Option<String>,
}

impl ListReference {
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        let (depth, list_id) = match raw.split_once(';') {
            Some((head, tail)) => (head.parse::<u32>().ok(), Some(tail.to_string())),
            None => (None, None),
        };
        Some(Self {
            raw: raw.to_string(),
            depth,
            list_id,
        })
    }
}

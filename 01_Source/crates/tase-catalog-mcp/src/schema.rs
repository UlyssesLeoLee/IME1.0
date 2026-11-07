//! The JSON Schema of the four MCP tools (detailed design §19.6, basic design §17.5).
//!
//! The schemas are written by hand as JSON text and are parsed with the strict parser of
//! `pime-core` - **no `serde`**, no third-party dependency. Every schema is a JSON Schema of the
//! draft 2020-12, and every schema is exported as a `&'static str`, so that the host that owns the
//! MCP session can hand it to a client without copying it.
//!
//! # The four tools
//!
//! | the tool | what it does | the required fields |
//! |---|---|---|
//! | `catalog.search` | finds the knowledge cards by keyword, language, layer, category and tag | `query`, `language` |
//! | `catalog.recommend` | ranks the assets that answer an intent, with the score broken down | `intent`, `language` |
//! | `catalog.instantiate` | drafts a plan out of an **approved** implementation of the target language | `asset_id`, `version`, `language` |
//! | `catalog.describe` | gives the whole knowledge card of one asset | `asset_id` |
//!
//! The required fields of `catalog.search` are the ones of the tool schema of detailed design
//! §19.6 (`"required": ["query", "language"]`): a caller that does not state the language of the
//! implementation it cares about gets a card that may not answer for it.
//!
//! # The strictness
//!
//! Every schema ends with `"additionalProperties": false`: a parameter that the schema does not
//! name is refused with [`ErrorCode::InvalidArgument`] rather than ignored. A model that guesses a
//! parameter learns that the guess was wrong instead of believing that it was honoured.
//!
//! [`ErrorCode::InvalidArgument`]: tase_contract::ErrorCode::InvalidArgument

/// The dialect of every schema of the crate: JSON Schema draft 2020-12.
pub const SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The schema of `catalog.search`.
pub const SEARCH_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "title": "Search the knowledge cards of the unified standard asset catalog",
  "description": "Find the knowledge cards of the catalog by keyword, language, layer, category and tag. Read-only: it writes no file and instantiates nothing. Every hit carries its counterexamples, its license and its risks.",
  "properties": {
    "query": {
      "type": "string",
      "minLength": 1,
      "maxLength": 300,
      "description": "The words to look for: the name, the alias, the id or the intent of the card. Chinese is accepted."
    },
    "language": {
      "type": "string",
      "minLength": 1,
      "maxLength": 32,
      "description": "The language of the implementation the caller cares about (rust, python, typescript, ...). A card that has no implementation at all is language neutral and matches every language."
    },
    "layer": {
      "type": "string",
      "enum": ["PATTERN", "ARCHITECTURE", "INTEGRATION", "RESILIENCE", "CONCURRENCY", "ALGORITHM"],
      "description": "Only the cards of this layer."
    },
    "category": {
      "type": "string",
      "maxLength": 64,
      "description": "Only the cards of this category (graph, sorting, creational, ...)."
    },
    "tag": {
      "type": "string",
      "maxLength": 128,
      "description": "Only the cards that carry this tag (stability, thread_safe, alternative=algorithm.graph.prim, ...)."
    },
    "limit": {
      "type": "integer",
      "minimum": 1,
      "maximum": 50,
      "description": "How many cards to answer with at most. The default is 10."
    }
  },
  "required": ["query", "language"],
  "additionalProperties": false
}"#;

/// The schema of `catalog.recommend`.
pub const RECOMMEND_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "title": "Recommend the assets that answer an intent",
  "description": "Rank the knowledge cards that answer an intent in a target language. Every candidate carries the breakdown of its score, its counterexamples, its license and its risks. Read-only: it writes no file and instantiates nothing.",
  "properties": {
    "intent": {
      "type": "string",
      "minLength": 1,
      "maxLength": 300,
      "description": "What the caller wants to do, in words (for example: a stable sort that keeps the order of the equal keys)."
    },
    "language": {
      "type": "string",
      "minLength": 1,
      "maxLength": 32,
      "description": "The language of the implementation the caller wants (rust, python, typescript, ...)."
    },
    "layer": {
      "type": "string",
      "enum": ["PATTERN", "ARCHITECTURE", "INTEGRATION", "RESILIENCE", "CONCURRENCY", "ALGORITHM"],
      "description": "Only the assets of this layer."
    },
    "category": {
      "type": "string",
      "maxLength": 64,
      "description": "Only the assets of this category."
    },
    "limit": {
      "type": "integer",
      "minimum": 1,
      "maximum": 50,
      "description": "How many candidates to answer with at most. The default is 10."
    },
    "constraints": {
      "type": "object",
      "description": "The conditions the caller imposes. An asset that does not answer them is not recommended, and the refusal says why.",
      "properties": {
        "max_complexity": {
          "type": "string",
          "enum": ["LOGARITHMIC", "LINEAR", "LINEARITHMIC", "QUADRATIC", "UNBOUNDED"],
          "description": "The largest cost the caller accepts."
        },
        "allow_global_mutable_state": {
          "type": "boolean",
          "description": "Whether the caller accepts a card that keeps a global mutable state. The default is true."
        },
        "require_thread_safety": {
          "type": "boolean",
          "description": "Whether the caller needs an asset that may be used from several threads at once. The default is false."
        },
        "allow_negative_inputs": {
          "type": "boolean",
          "description": "Whether the caller accepts a card that may need negative inputs. The default is true."
        },
        "require_stable_order": {
          "type": "boolean",
          "description": "Whether the caller needs an asset whose order is stable. The default is false."
        },
        "require_instantiable": {
          "type": "boolean",
          "description": "Whether the caller only accepts an asset that has an APPROVED implementation in the target language. The default is false."
        },
        "allowed_licenses": {
          "type": "array",
          "items": { "type": "string" },
          "description": "The licenses the caller accepts. An empty list states no restriction at the gate of the recommendation, but it never allows an instantiation."
        }
      },
      "additionalProperties": false
    }
  },
  "required": ["intent", "language"],
  "additionalProperties": false
}"#;

/// The schema of `catalog.instantiate`.
pub const INSTANTIATE_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "title": "Draft a plan out of an approved implementation",
  "description": "Draft the plan of an instantiation. It is allowed only when the asset has an APPROVED implementation in the target language; otherwise the tool answers POLICY_REJECTED and says why. It writes no file: the plan is a draft, and applying it is another tool.",
  "properties": {
    "asset_id": {
      "type": "string",
      "minLength": 1,
      "maxLength": 128,
      "description": "The id of the asset (algorithm.graph.kruskal)."
    },
    "version": {
      "type": "string",
      "minLength": 1,
      "maxLength": 32,
      "description": "The version of the asset the caller wants (1.0.0)."
    },
    "language": {
      "type": "string",
      "minLength": 1,
      "maxLength": 32,
      "description": "The language of the implementation to instantiate (rust, python, typescript, ...)."
    },
    "parameters": {
      "type": "object",
      "description": "The parameters of the instantiation, as the schema of the implementation declares them."
    },
    "allowed_licenses": {
      "type": "array",
      "items": { "type": "string" },
      "description": "The licenses the caller accepts. The list may not be empty: an unknown license is not a free license."
    }
  },
  "required": ["asset_id", "version", "language"],
  "additionalProperties": false
}"#;

/// The schema of `catalog.describe`.
pub const DESCRIBE_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "title": "Describe one asset of the catalog",
  "description": "Give the whole knowledge card of one asset: the body of its card.md, its manifest, its state and the state of its implementations. Read-only: it writes no file.",
  "properties": {
    "asset_id": {
      "type": "string",
      "minLength": 1,
      "maxLength": 128,
      "description": "The id of the asset (algorithm.graph.kruskal). An alias (MST) and a name are accepted as well."
    }
  },
  "required": ["asset_id"],
  "additionalProperties": false
}"#;

/// Every schema of the crate, with the name of the tool it belongs to.
pub const SCHEMAS: [(&str, &str); 4] = [
    ("catalog.search", SEARCH_SCHEMA),
    ("catalog.recommend", RECOMMEND_SCHEMA),
    ("catalog.instantiate", INSTANTIATE_SCHEMA),
    ("catalog.describe", DESCRIBE_SCHEMA),
];

/// The schema of a tool, by its name. `None` when the name is not a tool of the crate.
pub fn schema_of(name: &str) -> Option<&'static str> {
    SCHEMAS
        .iter()
        .find(|(tool, _)| *tool == name)
        .map(|(_, schema)| *schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::json::{Json, Limits, parse};

    /// The schema of a tool, parsed.
    fn parsed(name: &str) -> Json {
        let text = schema_of(name).unwrap_or_else(|| panic!("the tool {name} must have a schema"));
        parse(text, Limits::DEFAULT).unwrap_or_else(|error| panic!("the schema of {name} is not JSON: {error}"))
    }

    /// The value of a field of an object.
    fn field<'a>(value: &'a Json, key: &str) -> &'a Json {
        value
            .get(key)
            .unwrap_or_else(|| panic!("the field {key:?} must be there"))
    }

    #[test]
    fn the_four_schemas_are_json_objects() {
        assert_eq!(SCHEMAS.len(), 4);
        for (name, text) in SCHEMAS {
            let value = parse(text, Limits::DEFAULT)
                .unwrap_or_else(|error| panic!("the schema of {name} is not JSON: {error}"));
            assert!(matches!(value, Json::Obj(_)), "{name}");
        }
    }

    #[test]
    fn every_schema_is_a_draft_2020_12_schema() {
        for (name, _) in SCHEMAS {
            let value = parsed(name);
            assert_eq!(
                field(&value, "$schema").as_str(),
                Some(SCHEMA_DIALECT),
                "{name}"
            );
            assert_eq!(field(&value, "type").as_str(), Some("object"), "{name}");
        }
    }

    #[test]
    fn every_schema_declares_its_properties_and_refuses_the_others() {
        for (name, _) in SCHEMAS {
            let value = parsed(name);
            assert!(field(&value, "properties").as_object().is_some(), "{name}");
            assert_eq!(
                field(&value, "additionalProperties").as_bool(),
                Some(false),
                "{name}"
            );
            assert!(field(&value, "required").as_array().is_some(), "{name}");
        }
    }

    #[test]
    fn the_search_schema_asks_for_a_query_and_a_language() {
        let value = parsed("catalog.search");
        let required = field(&value, "required").as_array().expect("the required fields");
        let names: Vec<&str> = required.iter().filter_map(Json::as_str).collect();
        assert_eq!(names, ["query", "language"]);
        let properties = field(&value, "properties");
        assert!(properties.get("query").is_some());
        assert!(properties.get("language").is_some());
        assert!(properties.get("limit").is_some());
        assert!(properties.get("layer").is_some());
        assert!(properties.get("category").is_some());
        assert!(properties.get("tag").is_some());
    }

    #[test]
    fn the_recommend_schema_asks_for_an_intent_and_a_language() {
        let value = parsed("catalog.recommend");
        let required = field(&value, "required").as_array().expect("the required fields");
        let names: Vec<&str> = required.iter().filter_map(Json::as_str).collect();
        assert_eq!(names, ["intent", "language"]);
        let properties = field(&value, "properties");
        assert!(properties.get("constraints").is_some());
        let constraints = field(properties, "constraints");
        assert_eq!(
            field(constraints, "additionalProperties").as_bool(),
            Some(false)
        );
        let constraint_properties = field(constraints, "properties");
        for name in [
            "max_complexity",
            "allow_global_mutable_state",
            "require_thread_safety",
            "allow_negative_inputs",
            "require_stable_order",
            "require_instantiable",
            "allowed_licenses",
        ] {
            assert!(constraint_properties.get(name).is_some(), "{name}");
        }
    }

    #[test]
    fn the_instantiate_schema_asks_for_an_asset_a_version_and_a_language() {
        let value = parsed("catalog.instantiate");
        let required = field(&value, "required").as_array().expect("the required fields");
        let names: Vec<&str> = required.iter().filter_map(Json::as_str).collect();
        assert_eq!(names, ["asset_id", "version", "language"]);
        let properties = field(&value, "properties");
        assert!(properties.get("parameters").is_some());
        assert!(properties.get("allowed_licenses").is_some());
    }

    #[test]
    fn the_describe_schema_asks_for_an_asset() {
        let value = parsed("catalog.describe");
        let required = field(&value, "required").as_array().expect("the required fields");
        let names: Vec<&str> = required.iter().filter_map(Json::as_str).collect();
        assert_eq!(names, ["asset_id"]);
        let properties = field(&value, "properties");
        assert!(properties.get("asset_id").is_some());
    }

    #[test]
    fn the_layer_of_the_schema_is_an_enum_of_the_six_layers() {
        for name in ["catalog.search", "catalog.recommend"] {
            let value = parsed(name);
            let properties = field(&value, "properties");
            let layer = field(properties, "layer");
            let items = field(layer, "enum").as_array().expect("the layers");
            let names: Vec<&str> = items.iter().filter_map(Json::as_str).collect();
            assert_eq!(
                names,
                [
                    "PATTERN",
                    "ARCHITECTURE",
                    "INTEGRATION",
                    "RESILIENCE",
                    "CONCURRENCY",
                    "ALGORITHM"
                ],
                "{name}"
            );
        }
    }

    #[test]
    fn the_limit_of_the_schema_is_between_one_and_fifty() {
        for name in ["catalog.search", "catalog.recommend"] {
            let value = parsed(name);
            let properties = field(&value, "properties");
            let limit = field(properties, "limit");
            assert_eq!(field(limit, "type").as_str(), Some("integer"), "{name}");
            assert_eq!(field(limit, "minimum").as_i64(), Some(1), "{name}");
            assert_eq!(field(limit, "maximum").as_i64(), Some(50), "{name}");
        }
    }

    #[test]
    fn a_schema_of_an_unknown_tool_is_not_invented() {
        assert!(schema_of("catalog.nothing").is_none());
        assert!(schema_of("").is_none());
        assert!(schema_of("catalog.Search").is_none());
    }
}

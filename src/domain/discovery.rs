//! What `search` matches: the terms of a query against a unit's declared
//! metadata.
//!
//! Matching is literal, never semantic, and depends only on its inputs, so
//! the same index always gives the same results (see
//! `docs/domain/registries-and-discovery.md`, Provisional (D-32)).

use std::fmt;

use super::UnitIdentity;

/// The terms of a search, each matched on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Lowercase, without empty terms, in the order given.
    terms: Vec<String>,
}

impl SearchQuery {
    /// The query made of the whitespace-separated terms of `text`. A query
    /// with no term is an error, since it would match every unit.
    pub fn parse(text: &str) -> Result<Self, EmptyQuery> {
        let terms: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        if terms.is_empty() {
            Err(EmptyQuery)
        } else {
            Ok(Self { terms })
        }
    }

    /// Whether every term occurs, ignoring case, in the unit's identity or
    /// in its description. A unit without a known description is matched on
    /// its identity alone.
    pub fn matches(&self, identity: &UnitIdentity, description: Option<&str>) -> bool {
        let identity = identity.to_string().to_lowercase();
        let description = description.map(str::to_lowercase);
        self.terms.iter().all(|term| {
            identity.contains(term.as_str())
                || description
                    .as_deref()
                    .is_some_and(|text| text.contains(term.as_str()))
        })
    }
}

/// A search query with no term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyQuery;

impl fmt::Display for EmptyQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the search has no term")
    }
}

impl std::error::Error for EmptyQuery {}

#[cfg(test)]
mod tests {
    use super::{EmptyQuery, SearchQuery};
    use crate::domain::UnitIdentity;

    fn identity(text: &str) -> UnitIdentity {
        UnitIdentity::parse(text).unwrap()
    }

    #[test]
    fn a_query_needs_a_term() {
        assert_eq!(SearchQuery::parse(""), Err(EmptyQuery));
        assert_eq!(SearchQuery::parse(" \t\n"), Err(EmptyQuery));
    }

    #[test]
    fn terms_match_the_identity_or_the_description_ignoring_case() {
        let worker = identity("@metamao/cloudflare-worker");
        let description = Some("Deploys an Angular app to Cloudflare Workers.");
        for query in [
            "cloudflare",
            "CLOUDFLARE",
            "metamao/",
            "@metamao",
            "angular",
        ] {
            let query = SearchQuery::parse(query).unwrap();
            assert!(query.matches(&worker, description), "{query:?}");
        }
        assert!(
            !SearchQuery::parse("neon")
                .unwrap()
                .matches(&worker, description)
        );
    }

    #[test]
    fn every_term_must_match_somewhere() {
        let worker = identity("@metamao/cloudflare-worker");
        let description = Some("Deploys an Angular app.");
        let both = SearchQuery::parse("worker  angular").unwrap();
        assert!(both.matches(&worker, description));
        let one = SearchQuery::parse("worker docker").unwrap();
        assert!(!one.matches(&worker, description));
    }

    #[test]
    fn without_a_description_only_the_identity_is_matched() {
        let node = identity("@acme/node");
        assert!(SearchQuery::parse("node").unwrap().matches(&node, None));
        assert!(!SearchQuery::parse("runtime").unwrap().matches(&node, None));
    }
}

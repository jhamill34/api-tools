//! Shared helpers for the `OpenAPI` loader: extracting typed fields from raw
//! JSON, and resolving `$ref` references (internal and external, with
//! cycle detection and per-document caching).

use core::str::FromStr;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    io,
};

use serde::de::DeserializeOwned;

use crate::Fetcher;

use super::error;

/// The JSON key an `OpenAPI` reference object stores its target under.
const REF_KEY: &str = "$ref";

/// TODO: Why are we cloning?
pub fn required_field<T: DeserializeOwned>(
    current: &serde_json::Value,
    field: &str,
) -> error::Result<T> {
    let result = serde_json::from_value(
        current
            .get(field)
            .ok_or(error::ServiceLoader::MissingRequiredField(field.to_owned()))?
            .clone(),
    )?;
    Ok(result)
}

/// TODO: Why are we cloning?
pub fn default_field<T: DeserializeOwned + Default>(
    current: &serde_json::Value,
    field: &str,
) -> error::Result<T> {
    let result = match current.get(field) {
        Some(value) => serde_json::from_value(value.clone())?,
        None => Default::default(),
    };

    Ok(result)
}

/// TODO: Why are we cloning?
pub fn optional_field<T: DeserializeOwned>(
    current: &serde_json::Value,
    field: &str,
) -> error::Result<Option<T>> {
    let result = match current.get(field) {
        Some(value) => Some(serde_json::from_value(value.clone())?),
        None => None,
    };

    Ok(result)
}

/// If `item` is a `$ref` object, resolves it (recursively, in case the
/// target is itself a reference) against `root` — fetching and caching the
/// target document first if the reference is external — and returns the
/// resolved key and value. Returns `None` if `item` isn't a reference.
/// Errors on a reference cycle, tracked via `seen`.
pub fn handle_reference<R: io::Read>(
    item: &serde_json::Value,
    root: &serde_json::Value,
    fetcher: &dyn Fetcher<R>,
    cache: &mut HashMap<String, serde_json::Value>,
    seen: &mut HashSet<String>,
) -> error::Result<Option<(String, serde_json::Value)>> {
    let reference = optional_field::<String>(item, REF_KEY)?;
    if let Some(ref_key) = reference {
        if seen.contains(&ref_key) {
            return Err(error::ServiceLoader::CyclicalReference(ref_key));
        }
        seen.insert(ref_key.clone());

        let reference = ref_key.parse::<Reference>()?;

        let result = match reference.type_ {
            // NOTE: This clone shows up as a low grade number of allocs... this was an explicit
            // choice because we can't return the nested reference without fighting the borrow
            // checker. We might be able to get away with Rc if we really needed to
            ReferenceType::Internal => {
                let result = reference.path.resolve(root)?.clone();
                handle_reference(&result, root, fetcher, cache, seen)?
                    .unwrap_or((ref_key.clone(), result))
            }
            ReferenceType::External(source) => {
                let external = fetch_and_cache(&source, fetcher, cache)?.clone();
                let result = reference.path.resolve(&external)?.clone();

                handle_reference(&result, &external, fetcher, cache, &mut HashSet::new())?
                    .unwrap_or((ref_key.clone(), result))
            }
        };

        seen.remove(&ref_key);
        Ok(Some(result))
    } else {
        Ok(None)
    }
}

/// Fetches and parses `source` as YAML the first time it's requested,
/// caching the result in `cache` for subsequent lookups.
fn fetch_and_cache<'cache, R: io::Read>(
    source: &str,
    fetcher: &dyn Fetcher<R>,
    cache: &'cache mut HashMap<String, serde_json::Value>,
) -> error::Result<&'cache serde_json::Value> {
    let result = match cache.entry(source.to_owned()) {
        Entry::Vacant(vacant) => {
            let result = fetcher.fetch(source)?;
            let result: serde_json::Value = serde_yaml::from_reader(result)?;
            vacant.insert(result)
        }
        Entry::Occupied(occupied) => occupied.into_mut(),
    };

    Ok(result)
}

/// A parsed `$ref` string: `"{source}#{path}"`, where an empty `source`
/// means the reference targets the current document.
#[derive(Debug)]
struct Reference {
    /// The JSON pointer into the target document.
    pub path: jsonptr::Pointer,

    /// Whether the reference targets the current document or another one.
    pub type_: ReferenceType,
}

/// Whether a [`Reference`] targets the document it appears in, or another
/// document fetched by URL/path.
#[derive(Debug)]
enum ReferenceType {
    /// Targets the current document.
    Internal,

    /// Targets another document, fetched from this source.
    External(String),
}

impl FromStr for Reference {
    type Err = error::ServiceLoader;

    fn from_str(string: &str) -> Result<Self, Self::Err> {
        let (source, path) = string
            .split_once('#')
            .ok_or_else(|| error::ServiceLoader::NotFound("Json Path Fragment".into()))?;

        let path = path.parse::<jsonptr::Pointer>()?;

        if source.is_empty() {
            Ok(Self {
                path,
                type_: ReferenceType::Internal,
            })
        } else {
            Ok(Self {
                path,
                type_: ReferenceType::External(source.to_owned()),
            })
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_internal_reference_parsing() -> error::Result<()> {
        let reference = "#/components/schemas/Test";
        let reference = reference.parse::<Reference>()?;

        assert!(matches!(reference.type_, ReferenceType::Internal));
        Ok(())
    }

    #[test]
    fn test_external_reference_parsing() -> error::Result<()> {
        let reference = "https://example.com/json#/components/schemas/Test";
        let reference = reference.parse::<Reference>()?;

        match reference.type_ {
            ReferenceType::External(e) => {
                assert_eq!("https://example.com/json", e);
            }
            ReferenceType::Internal => unreachable!(),
        }
        Ok(())
    }

    #[test]
    fn test_reference_without_a_fragment_is_rejected() {
        let result = "https://example.com/json".parse::<Reference>();

        assert!(
            matches!(result, Err(error::ServiceLoader::NotFound(_))),
            "expected NotFound, got {result:?}"
        );
    }
}

//!

use core::str::FromStr;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    io,
};

use serde::de::DeserializeOwned;

use crate::Fetcher;

use super::error;

///
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

///
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

///
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

///
#[derive(Debug)]
struct Reference {
    ///
    pub path: jsonptr::Pointer,

    ///
    pub type_: ReferenceType,
}

///
#[derive(Debug)]
enum ReferenceType {
    ///
    Internal,

    ///
    External(String),
}

impl FromStr for Reference {
    type Err = error::ServiceLoader;

    fn from_str(string: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = string.split('#').collect();
        let source = parts.first();

        let path = parts
            .get(1)
            .ok_or_else(|| error::ServiceLoader::NotFound("Json Path Fragment".into()))?;

        let path = path.parse::<jsonptr::Pointer>()?;

        if let Some(source) = source {
            if source.is_empty() {
                Ok(Self {
                    path,
                    type_: ReferenceType::Internal,
                })
            } else {
                Ok(Self {
                    path,
                    type_: ReferenceType::External((*source).to_owned()),
                })
            }
        } else {
            Ok(Self {
                path,
                type_: ReferenceType::Internal,
            })
        }
    }
}

#[cfg(test)]
mod test {
    #![allow(clippy::restriction, clippy::pedantic)]
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
            _ => unreachable!(),
        }
        Ok(())
    }
}

#![allow(clippy::arithmetic_side_effects)]

//!

use std::collections::HashMap;

///
#[non_exhaustive]
pub struct Config {
    ///
    pub single_wildcard_byte: u8,

    ///
    pub multi_wildcard_byte: u8,
}

///
#[non_exhaustive]
pub struct Trie<'trie, T> {
    ///
    pub value: Option<T>,

    ///
    pub children: HashMap<u8, Trie<'trie, T>>,

    ///
    pub config: &'trie Config,
}

impl<'trie, T> Default for Trie<'trie, T> {
    #[inline]
    fn default() -> Self {
        Self::new(&Config {
            single_wildcard_byte: b'x',
            multi_wildcard_byte: b'*',
        })
    }
}

impl<'trie, T> Trie<'trie, T> {
    ///
    #[inline]
    #[must_use]
    pub fn new(config: &'trie Config) -> Self {
        Self {
            value: None,
            children: HashMap::new(),
            config,
        }
    }

    ///
    #[inline]
    pub fn insert(&mut self, key: &str, value: T) {
        let chars = key.as_bytes();
        self.insert_bytes(chars, value);
    }

    ///
    #[inline]
    fn insert_bytes(&mut self, key: &[u8], value: T) {
        if let Some(next) = key.first() {
            let child = self
                .children
                .entry(*next)
                .or_insert_with(|| Trie::new(self.config));

            let remainder = key.get(1..).unwrap_or_default();
            child.insert_bytes(remainder, value);
        } else {
            self.value = Some(value);
        }
    }

    ///
    #[inline]
    pub fn find(&self, key: &str) -> Option<&T> {
        let chars = key.as_bytes();
        self.find_bytes(chars)
    }

    ///
    #[inline]
    fn find_bytes(&self, key: &[u8]) -> Option<&T> {
        if let Some(next) = key.first() {
            let remainder = key.get(1..).unwrap_or_default();

            let found = self
                .children
                .get(next)
                .or_else(|| self.children.get(&self.config.single_wildcard_byte))
                .and_then(|child| child.find_bytes(remainder));

            match found {
                None => {
                    if let Some(wildcard) = self.children.get(&self.config.multi_wildcard_byte) {
                        let wildcard_child = key.iter().enumerate().find_map(|(index, val)| {
                            wildcard.children.get(val).map(|child| (index, child))
                        });

                        if let Some((index, wildcard_child)) = wildcard_child {
                            if let Some(remainder) = key.get((index + 1)..) {
                                wildcard_child.find_bytes(remainder)
                            } else {
                                wildcard_child.value.as_ref()
                            }
                        } else {
                            wildcard.value.as_ref()
                        }
                    } else {
                        None
                    }
                }
                _ => found,
            }
        } else {
            self.value.as_ref()
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn simple_test() {
        let mut root = Trie::<u32>::default();

        root.insert("*/*", 30);
        root.insert("application/json", 10);
        root.insert("application/*", 20);

        assert_eq!(None, root.find("app"));
        assert_eq!(None, root.find("applier"));
        assert_eq!(Some(10), root.find("application/json").copied());
        assert_eq!(Some(20), root.find("application/csv").copied());
        assert_eq!(Some(30), root.find("app/csv").copied());
    }

    #[test]
    fn test_wildcard() {
        let mut root = Trie::<u32>::default();

        root.insert("2xx", 20);

        assert_eq!(None, root.find("300"));
        assert_eq!(Some(20), root.find("201").copied());
        assert_eq!(None, root.find("2010").copied());
    }

    #[test]
    fn test_wildcard_2() {
        let mut root = Trie::<u32>::default();

        root.insert("2x1", 20);

        assert_eq!(None, root.find("300"));
        assert_eq!(Some(20), root.find("201").copied());
        assert_eq!(None, root.find("2010").copied());
    }
}

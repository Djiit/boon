use std::collections::{HashMap, HashSet};

use crate::{
    compiler::CompileError,
    draft::*,
    util::{self, *},
};

use serde_json::Value;
use url::Url;

pub(crate) struct Root {
    pub(crate) draft: &'static Draft,
    pub(crate) resources: HashMap<String, Resource>, // ptr => _
    pub(crate) url: Url,
    pub(crate) doc: Value,
    pub(crate) meta_vocabs: Option<Vec<String>>,
}

impl Root {
    pub(crate) fn has_vocab(&self, name: &str) -> bool {
        if self.draft.version < 2019 || name == "core" {
            return true;
        }
        if let Some(vocabs) = &self.meta_vocabs {
            return vocabs.iter().any(|s| s == name);
        }
        self.draft.default_vocabs.contains(&name)
    }

    pub(crate) fn check_duplicate_id(&self) -> Result<(), CompileError> {
        let mut map = HashMap::new();
        for (ptr, Resource { id, .. }) in &self.resources {
            if let Some(ptr2) = map.insert(id, ptr) {
                return Err(CompileError::DuplicateId {
                    url: self.url.as_str().to_owned(),
                    id: id.as_str().to_owned(),
                    ptr1: ptr.to_owned(),
                    ptr2: ptr2.to_owned(),
                });
            }
        }
        Ok(())
    }

    // resolves `loc` to root-url#json-pointer
    pub(crate) fn resolve(&self, loc: &str) -> Result<String, CompileError> {
        let (url, frag) = split(loc);

        let (res_ptr, res) = {
            if url == self.url.as_str() {
                let res = self.resources.get("").ok_or(CompileError::Bug(
                    format!("no root resource found for {url}").into(),
                ))?;
                ("", res)
            } else {
                // look for resource with id==url
                let entry = self
                    .resources
                    .iter()
                    .find(|(_res_ptr, res)| res.id.as_str() == url);

                match entry {
                    Some((ptr, res)) => (ptr.as_str(), res),
                    _ => return Ok(loc.to_owned()), // external url
                }
            }
        };

        let anchor = frag.to_anchor().map_err(|e| CompileError::ParseUrlError {
            url: loc.to_owned(),
            src: e.into(),
        })?;

        if let Some(anchor) = anchor {
            if let Some(anchor_ptr) = res.anchors.get(anchor.as_ref()) {
                Ok(format!("{}#{}", self.url, percent_encode(anchor_ptr)))
            } else {
                Err(CompileError::AnchorNotFound {
                    url: self.url.as_str().to_owned(),
                    reference: loc.to_owned(),
                })
            }
        } else {
            Ok(format!("{}#{}{}", self.url, percent_encode(res_ptr), frag))
        }
    }

    pub(crate) fn resource(&self, mut ptr: &str) -> Option<&Resource> {
        loop {
            if let Some(res) = self.resources.get(ptr) {
                return Some(res);
            }
            let Some(slash) = ptr.rfind('/') else {
                break;
            };
            ptr = &ptr[..slash];
        }
        None
    }

    pub(crate) fn base_url(&self, ptr: &str) -> &Url {
        if let Some(Resource { id, .. }) = self.resource(ptr) {
            return id;
        }
        &self.url
    }

    pub(crate) fn lookup_ptr(&self, ptr: &str) -> Result<Option<&Value>, ()> {
        util::lookup_ptr(&self.doc, ptr)
    }

    pub(crate) fn get_reqd_vocabs(&self) -> Result<Option<Vec<String>>, CompileError> {
        if self.draft.version < 2019 {
            return Ok(None);
        }
        let Value::Object(obj) = &self.doc else {
            return Ok(None);
        };

        let Some(Value::Object(obj)) = obj.get("$vocabulary") else {
            return Ok(None);
        };

        let mut vocabs = vec![];
        for (vocab, reqd) in obj {
            if let Value::Bool(true) = reqd {
                let name = vocab
                    .strip_prefix(self.draft.vocab_prefix)
                    .filter(|name| self.draft.all_vocabs.contains(name));
                if let Some(name) = name {
                    vocabs.push(name.to_owned()); // todo: avoid alloc
                } else {
                    return Err(CompileError::UnsupprtedVocabulary {
                        url: self.url.as_str().to_owned(),
                        vocabulary: vocab.to_owned(),
                    });
                }
            }
        }
        Ok(Some(vocabs))
    }

    pub(crate) fn add_subschema(&mut self, ptr: &str) -> Result<(), CompileError> {
        let v = util::lookup_ptr(&self.doc, ptr).map_err(|_| {
            CompileError::InvalidJsonPointer(format!("{}#{}", self.url, percent_encode(ptr)))
        })?;
        let Some(v) = v else {
            let loc = format!("{}#{}", self.url, percent_encode(ptr));
            return Err(CompileError::JsonPointerNotFound(loc))?;
        };
        let base_url = self.base_url(ptr).clone();
        self.draft.collect_resources(
            v,
            &base_url,
            ptr.to_string(),
            &self.url,
            &mut self.resources,
        )?;
        if !self.resources.contains_key(ptr) {
            if let Some(res) = self.resource(ptr) {
                if let Some(res) = self.resources.get_mut(&res.ptr.to_string()) {
                    self.draft
                        .collect_anchors(v, ptr.as_ref(), res, &self.url)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct Resource {
    pub(crate) ptr: String, // from root
    pub(crate) id: Url,
    pub(crate) anchors: HashMap<String, String>, // anchor => ptr
    pub(crate) dynamic_anchors: HashSet<String>,
}

impl Resource {
    pub(crate) fn new(ptr: String, id: Url) -> Self {
        Self {
            ptr,
            id,
            anchors: HashMap::new(),
            dynamic_anchors: HashSet::new(),
        }
    }
}

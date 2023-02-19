mod compiler;
mod content;
mod draft;
mod formats;
mod loader;
mod root;
mod roots;
mod util;

pub use compiler::Draft;
pub use compiler::*;
use content::{Decoder, MediaType};
use formats::Format;
pub use loader::*;

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
};

use regex::Regex;
use serde_json::{Number, Value};
use util::*;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaIndex(usize);

#[derive(Default)]
pub struct Schemas {
    list: Vec<Schema>,
    map: HashMap<String, usize>, // loc => schema-index
}

impl Schemas {
    pub fn new() -> Self {
        Self::default()
    }

    fn enqueue(&self, queue: &mut VecDeque<String>, mut loc: String) -> usize {
        if loc.rfind('#').is_none() {
            loc.push('#');
        }

        if let Some(&index) = self.map.get(&loc) {
            // already got compiled
            return index;
        }
        if let Some(qindex) = queue.iter().position(|e| *e == loc) {
            // already queued for compilation
            return self.list.len() + qindex;
        }

        // new compilation request
        queue.push_back(loc);
        self.list.len() + queue.len() - 1
    }

    fn insert(&mut self, loc: String, sch: Schema) -> SchemaIndex {
        let index = self.list.len();
        self.list.push(sch);
        self.map.insert(loc, index);
        SchemaIndex(index)
    }

    fn get(&self, index: usize) -> &Schema {
        &self.list[index] // todo: return bug
    }

    fn get_by_loc(&self, loc: &str) -> Option<&Schema> {
        let mut loc = Cow::from(loc);
        if loc.rfind('#').is_none() {
            let mut s = loc.into_owned();
            s.push('#');
            loc = Cow::from(s);
        }
        self.map.get(loc.as_ref()).and_then(|&i| self.list.get(i))
    }

    /// Returns true if `sch_index` is generated for this instance.
    pub fn contains(&self, sch_index: SchemaIndex) -> bool {
        self.list.get(sch_index.0).is_some()
    }

    /// Validates `v` with schema identified by `sch_index`
    ///
    /// # Panics
    ///
    /// Panics if `sch_index` is not generated for this instance.
    /// [`Schemas::contains`] can be used too ensure that it does not panic.
    pub fn validate(&self, v: &Value, sch_index: SchemaIndex) -> Result<(), ValidationError> {
        let Some(sch) = self.list.get(sch_index.0) else {
            panic!("Schemas::validate: schema index out of bounds");
        };
        let scope = Scope {
            sch: sch.index,
            kw_path: Cow::from(""),
            vid: 0,
            parent: None,
        };
        match sch.validate(v, String::new(), self, scope) {
            Err(e) => Err(ValidationError {
                keyword_location: String::new(),
                absolute_keyword_location: sch.loc.clone(),
                instance_location: String::new(),
                kind: ErrorKind::Schema {
                    url: sch.loc.clone(),
                },
                causes: vec![e],
            }),
            Ok(_) => Ok(()),
        }
    }
}

macro_rules! kind {
    ($kind:ident, $name:ident: $value:expr) => {
        ErrorKind::$kind { $name: $value }
    };
    ($kind:ident, $got:expr, $want:expr) => {
        ErrorKind::$kind {
            got: $got,
            want: $want,
        }
    };
    ($kind: ident) => {
        ErrorKind::$kind
    };
}

#[derive(Default)]
struct Schema {
    draft_version: usize,
    index: usize,
    loc: String,
    resource: usize,
    dynamic_anchors: HashMap<String, usize>,

    // type agnostic --
    boolean: Option<bool>, // boolean schema
    ref_: Option<usize>,
    recursive_ref: Option<usize>,
    recursive_anchor: bool,
    dynamic_ref: Option<usize>,
    dynamic_anchor: Option<String>,
    types: Vec<Type>,
    enum_: Vec<Value>,
    constant: Option<Value>,
    not: Option<usize>,
    all_of: Vec<usize>,
    any_of: Vec<usize>,
    one_of: Vec<usize>,
    if_: Option<usize>,
    then: Option<usize>,
    else_: Option<usize>,
    format: Option<(String, Format)>,

    // object --
    min_properties: Option<usize>,
    max_properties: Option<usize>,
    required: Vec<String>,
    properties: HashMap<String, usize>,
    pattern_properties: Vec<(Regex, usize)>,
    property_names: Option<usize>,
    additional_properties: Option<Additional>,
    dependent_required: HashMap<String, Vec<String>>,
    dependent_schemas: HashMap<String, usize>,
    dependencies: HashMap<String, Dependency>,
    unevaluated_properties: Option<usize>,

    // array --
    min_items: Option<usize>,
    max_items: Option<usize>,
    unique_items: bool,
    min_contains: Option<usize>,
    max_contains: Option<usize>,
    contains: Option<usize>,
    items: Option<Items>,
    additional_items: Option<Additional>,
    prefix_items: Vec<usize>,
    items2020: Option<usize>,
    unevaluated_items: Option<usize>,

    // string --
    min_length: Option<usize>,
    max_length: Option<usize>,
    pattern: Option<Regex>,
    content_encoding: Option<(String, Decoder)>,
    content_media_type: Option<(String, MediaType)>,

    // number --
    minimum: Option<Number>,
    maximum: Option<Number>,
    exclusive_minimum: Option<Number>,
    exclusive_maximum: Option<Number>,
    multiple_of: Option<Number>,
}

#[derive(Debug)]
enum Items {
    SchemaRef(usize),
    SchemaRefs(Vec<usize>),
}

#[derive(Debug)]
enum Additional {
    Bool(bool),
    SchemaRef(usize),
}

#[derive(Debug)]
enum Dependency {
    Props(Vec<String>),
    SchemaRef(usize),
}

#[derive(Default)]
struct Uneval<'v> {
    props: HashSet<&'v String>,
    items: HashSet<usize>,
}

impl<'v> Uneval<'v> {
    fn merge(&mut self, other: &Uneval) {
        self.props.retain(|p| other.props.contains(p));
        self.items.retain(|i| other.items.contains(i));
    }
}

impl<'v> From<&'v Value> for Uneval<'v> {
    fn from(v: &'v Value) -> Self {
        let mut uneval = Self::default();
        match v {
            Value::Object(obj) => uneval.props = obj.keys().collect(),
            Value::Array(arr) => uneval.items = (0..arr.len()).collect(),
            _ => (),
        }
        uneval
    }
}

#[derive(Debug, Default)]
struct Scope<'a> {
    sch: usize,
    kw_path: Cow<'static, str>,
    /// unique id of value being validated
    // if two scope validate same value, they will have same vid
    vid: usize,
    parent: Option<&'a Scope<'a>>,
}

impl<'a> Scope<'a> {
    fn child(sch: usize, kw_path: Cow<'static, str>, vid: usize, parent: &'a Scope) -> Self {
        Self {
            sch,
            kw_path,
            vid,
            parent: Some(parent),
        }
    }

    fn kw_loc(&self, kw_path: &str) -> String {
        let mut loc = kw_path.to_string();
        let mut scope = self;
        loop {
            if !loc.is_empty() {
                loc.insert(0, '/');
            }
            loc.insert_str(0, scope.kw_path.as_ref());
            if let Some(parent) = scope.parent {
                scope = parent;
            } else {
                break;
            }
        }
        loc
    }

    fn has_cycle(&self) -> bool {
        let mut scope = self.parent;
        while let Some(scp) = scope {
            if scp.vid != self.vid {
                break;
            }
            if scp.sch == self.sch {
                return true;
            }
            scope = scp.parent;
        }
        false
    }
}

impl Schema {
    fn new(loc: String) -> Self {
        Self {
            loc,
            ..Default::default()
        }
    }

    fn validate<'v>(
        &self,
        v: &'v Value,
        vloc: String,
        schemas: &Schemas,
        scope: Scope,
    ) -> Result<Uneval<'v>, ValidationError> {
        let error = |kw_path: &str, kind| ValidationError {
            keyword_location: scope.kw_loc(kw_path),
            absolute_keyword_location: match kw_path.is_empty() {
                true => self.loc.clone(),
                false => format!("{}/{kw_path}", self.loc),
            },
            instance_location: vloc.clone(),
            kind,
            causes: vec![],
        };

        let mut errors = vec![];
        macro_rules! add_error {
            ($kw_path:expr, $kind:expr) => {
                errors.push(error($kw_path, $kind))
            };
        }
        macro_rules! add_err {
            ($result:expr) => {
                if let Err(e) = $result {
                    errors.push(e);
                }
            };
        }

        if scope.has_cycle() {
            return Err(error("", kind!(RefCycle)));
        }

        let mut _uneval = Uneval::from(v);
        let uneval = &mut _uneval;
        let validate = |sch: usize, kw_path, v: &Value, vpath: &str| {
            let scope = Scope::child(sch, kw_path, scope.vid + 1, &scope);
            schemas
                .get(sch)
                .validate(v, format!("{vloc}/{vpath}"), schemas, scope)
                .map(|_| ())
        };
        let validate_self = |sch: usize, kw_path, uneval: &mut Uneval<'_>| {
            let scope = Scope::child(sch, kw_path, scope.vid, &scope);
            let result = schemas.get(sch).validate(v, vloc.clone(), schemas, scope);
            if let Ok(reply) = &result {
                uneval.merge(reply);
            }
            result.map(|_| ())
        };

        // boolean --
        if let Some(b) = self.boolean {
            if !b {
                return Err(error("", kind!(FalseSchema)));
            }
            return Ok(_uneval);
        }

        // type --
        if !self.types.is_empty() {
            let v_type = Type::of(v);
            let matched = self.types.iter().any(|t| {
                if *t == Type::Integer && v_type == Type::Number {
                    if let Value::Number(n) = v {
                        return n.is_i64()
                            || n.is_u64()
                            || n.as_f64().filter(|n| n.fract() == 0.0).is_some();
                    }
                }
                *t == v_type
            });
            if !matched {
                add_error!("type", kind!(Type, v_type, self.types.clone()));
            }
        }

        // enum --
        if !self.enum_.is_empty() && !self.enum_.iter().any(|e| equals(e, v)) {
            add_error!("enum", kind!(Enum, v.clone(), self.enum_.clone()));
        }

        // constant --
        if let Some(c) = &self.constant {
            if !equals(v, c) {
                add_error!("const", kind!(Const, v.clone(), c.clone()));
            }
        }

        // format --
        if let Some((format, check)) = &self.format {
            if !check(v) {
                add_error!("format", kind!(Format, v.clone(), format.clone()));
            }
        }

        match v {
            Value::Object(obj) => {
                // minProperties --
                if let Some(min) = self.min_properties {
                    if obj.len() < min {
                        add_error!("minProperties", kind!(MinProperties, obj.len(), min));
                    }
                }

                // maxProperties --
                if let Some(max) = self.max_properties {
                    if obj.len() > max {
                        add_error!("maxProperties", kind!(MaxProperties, obj.len(), max));
                    }
                }

                // required --
                let missing = self
                    .required
                    .iter()
                    .filter(|p| !obj.contains_key(p.as_str()))
                    .cloned()
                    .collect::<Vec<String>>();
                if !missing.is_empty() {
                    add_error!("required", kind!(Required, want: missing));
                }

                // dependencies --
                for (pname, dependency) in &self.dependencies {
                    if obj.contains_key(pname) {
                        let kw_path = format!("dependencies/{}", escape(pname));
                        match dependency {
                            Dependency::Props(required) => {
                                let missing = required
                                    .iter()
                                    .filter(|p| !obj.contains_key(p.as_str()))
                                    .cloned()
                                    .collect::<Vec<String>>();
                                if !missing.is_empty() {
                                    add_error!(
                                        &kw_path,
                                        kind!(DependentRequired, pname.clone(), missing)
                                    );
                                }
                            }
                            Dependency::SchemaRef(sch) => {
                                add_err!(validate_self(*sch, kw_path.into(), uneval));
                            }
                        }
                    }
                }

                // dependentSchemas --
                for (pname, sch) in &self.dependent_schemas {
                    if obj.contains_key(pname) {
                        let kw_path = format!("dependentSchemas/{}", escape(pname));
                        add_err!(validate_self(*sch, kw_path.into(), uneval));
                    }
                }

                // dependentRequired --
                for (pname, required) in &self.dependent_required {
                    if obj.contains_key(pname) {
                        let missing = required
                            .iter()
                            .filter(|p| !obj.contains_key(p.as_str()))
                            .cloned()
                            .collect::<Vec<String>>();
                        if !missing.is_empty() {
                            add_error!(
                                &format!("dependentRequired/{}", escape(pname)),
                                kind!(DependentRequired, pname.clone(), missing)
                            );
                        }
                    }
                }

                // properties --
                for (pname, &psch) in &self.properties {
                    if let Some(pvalue) = obj.get(pname) {
                        uneval.props.remove(pname);
                        let kw_path = format!("properties/{}", escape(pname));
                        add_err!(validate(psch, kw_path.into(), pvalue, &escape(pname)));
                    }
                }

                // patternProperties --
                for (regex, psch) in &self.pattern_properties {
                    for (pname, pvalue) in obj.iter().filter(|(pname, _)| regex.is_match(pname)) {
                        uneval.props.remove(pname);
                        let kw_path = format!("patternProperties/{}", escape(regex.as_str()));
                        add_err!(validate(*psch, kw_path.into(), pvalue, &escape(pname)));
                    }
                }

                // propertyNames --
                if let Some(sch) = &self.property_names {
                    for pname in obj.keys() {
                        let v = Value::String(pname.to_owned());
                        add_err!(validate(*sch, "propertyNames".into(), &v, &escape(pname)));
                    }
                }

                // additionalProperties --
                if let Some(additional) = &self.additional_properties {
                    let kw_path = "additionalProperties";
                    match additional {
                        Additional::Bool(allowed) => {
                            if !allowed && !uneval.props.is_empty() {
                                add_error!(
                                    kw_path,
                                    kind!(AdditionalProperties, got: uneval.props.iter().cloned().cloned().collect())
                                );
                            }
                        }
                        Additional::SchemaRef(sch) => {
                            for &pname in uneval.props.iter() {
                                if let Some(pvalue) = obj.get(pname) {
                                    add_err!(validate(
                                        *sch,
                                        kw_path.into(),
                                        pvalue,
                                        &escape(pname)
                                    ));
                                }
                            }
                        }
                    }
                    uneval.props.clear();
                }
            }

            Value::Array(arr) => {
                // minItems --
                if let Some(min) = self.min_items {
                    if arr.len() < min {
                        add_error!("minItems", kind!(MinItems, arr.len(), min));
                    }
                }

                // maxItems --
                if let Some(max) = self.max_items {
                    if arr.len() > max {
                        add_error!("maxItems", kind!(MaxItems, arr.len(), max));
                    }
                }

                // uniqueItems --
                if self.unique_items {
                    for i in 1..arr.len() {
                        for j in 0..i {
                            if equals(&arr[i], &arr[j]) {
                                add_error!("uniqueItems", kind!(UniqueItems, got: [j, i]));
                            }
                        }
                    }
                }

                // items --
                if let Some(items) = &self.items {
                    match items {
                        Items::SchemaRef(sch) => {
                            for (i, item) in arr.iter().enumerate() {
                                add_err!(validate(*sch, "items".into(), item, &i.to_string()));
                            }
                            uneval.items.clear();
                        }
                        Items::SchemaRefs(list) => {
                            for (i, (item, sch)) in arr.iter().zip(list).enumerate() {
                                uneval.items.remove(&i);
                                let kw_path = format!("items/{i}");
                                add_err!(validate(*sch, kw_path.into(), item, &i.to_string()));
                            }
                        }
                    }
                }

                // additionalItems --
                if let Some(additional) = &self.additional_items {
                    let kw_path = "additionalItems";
                    match additional {
                        Additional::Bool(allowed) => {
                            if !allowed && !uneval.items.is_empty() {
                                add_error!(
                                    kw_path,
                                    kind!(AdditionalItems, got: arr.len() - uneval.items.len())
                                );
                            }
                        }
                        Additional::SchemaRef(sch) => {
                            for &index in uneval.items.iter() {
                                if let Some(pvalue) = arr.get(index) {
                                    add_err!(validate(
                                        *sch,
                                        kw_path.into(),
                                        pvalue,
                                        &index.to_string(),
                                    ));
                                }
                            }
                        }
                    }
                    uneval.items.clear();
                }

                // prefixItems --
                for (i, (sch, item)) in self.prefix_items.iter().zip(arr).enumerate() {
                    uneval.items.remove(&i);
                    let kw_path = format!("prefixItems/{i}");
                    add_err!(validate(*sch, kw_path.into(), item, &i.to_string()));
                }

                // items2020 --
                if let Some(sch) = &self.items2020 {
                    for &index in uneval.items.iter() {
                        if let Some(pvalue) = arr.get(index) {
                            add_err!(validate(*sch, "items".into(), pvalue, &index.to_string()));
                        }
                    }
                    uneval.items.clear();
                }

                // contains --
                let mut contains_matched = vec![];
                let mut contains_errors = vec![];
                if let Some(sch) = &self.contains {
                    for (i, item) in arr.iter().enumerate() {
                        if let Err(e) = validate(*sch, "contains".into(), item, &i.to_string()) {
                            contains_errors.push(e);
                        } else {
                            contains_matched.push(i);
                            if self.draft_version >= 2020 {
                                uneval.items.remove(&i);
                            }
                        }
                    }
                }

                // minContains --
                if let Some(min) = self.min_contains {
                    if contains_matched.len() < min {
                        let mut e = error(
                            "minContains",
                            kind!(MinContains, contains_matched.clone(), min),
                        );
                        e.causes = contains_errors;
                        errors.push(e);
                    }
                } else if self.contains.is_some() && contains_matched.is_empty() {
                    let mut e = error("contains", kind!(Contains));
                    e.causes = contains_errors;
                    errors.push(e);
                }

                // maxContains --
                if let Some(max) = self.max_contains {
                    if contains_matched.len() > max {
                        add_error!("maxContains", kind!(MaxContains, contains_matched, max));
                    }
                }
            }

            Value::String(s) => {
                let mut len = None;

                // minLength --
                if let Some(min) = self.min_length {
                    let len = len.get_or_insert_with(|| s.chars().count());
                    if *len < min {
                        add_error!("minLength", kind!(MinLength, *len, min));
                    }
                }

                // maxLength --
                if let Some(max) = self.max_length {
                    let len = len.get_or_insert_with(|| s.chars().count());
                    if *len > max {
                        add_error!("maxLength", kind!(MaxLength, *len, max));
                    }
                }

                // pattern --
                if let Some(regex) = &self.pattern {
                    if !regex.is_match(s) {
                        add_error!(
                            "pattern",
                            kind!(Pattern, s.clone(), regex.as_str().to_string())
                        );
                    }
                }

                // contentEncoding --
                let mut decoded = Cow::from(s.as_bytes());
                if let Some((encoding, decode)) = &self.content_encoding {
                    match decode(s) {
                        Some(bytes) => decoded = Cow::from(bytes),
                        None => add_error!(
                            "contentEncoding",
                            kind!(ContentEncoding, s.clone(), encoding.clone())
                        ),
                    }
                }

                // contentMediaType --
                if let Some((media_type, check)) = &self.content_media_type {
                    if !check(decoded.as_ref()) {
                        add_error!(
                            "contentMediaType",
                            kind!(ContentMediaType, decoded.into_owned(), media_type.clone())
                        );
                    }
                }
            }

            Value::Number(n) => {
                // minimum --
                if let Some(min) = &self.minimum {
                    if let (Some(minf), Some(vf)) = (min.as_f64(), n.as_f64()) {
                        if vf < minf {
                            add_error!("minimum", kind!(Minimum, n.clone(), min.clone()));
                        }
                    }
                }

                // maximum --
                if let Some(max) = &self.maximum {
                    if let (Some(maxf), Some(vf)) = (max.as_f64(), n.as_f64()) {
                        if vf > maxf {
                            add_error!("maximum", kind!(Maximum, n.clone(), max.clone()));
                        }
                    }
                }

                // exclusiveMinimum --
                if let Some(ex_min) = &self.exclusive_minimum {
                    if let (Some(ex_minf), Some(nf)) = (ex_min.as_f64(), n.as_f64()) {
                        if nf <= ex_minf {
                            add_error!(
                                "exclusiveMinimum",
                                kind!(ExclusiveMinimum, n.clone(), ex_min.clone())
                            );
                        }
                    }
                }

                // exclusiveMaximum --
                if let Some(ex_max) = &self.exclusive_maximum {
                    if let (Some(ex_maxf), Some(nf)) = (ex_max.as_f64(), n.as_f64()) {
                        if nf >= ex_maxf {
                            add_error!(
                                "exclusiveMaximum",
                                kind!(ExclusiveMaximum, n.clone(), ex_max.clone())
                            );
                        }
                    }
                }

                // multipleOf --
                if let Some(mul) = &self.multiple_of {
                    if let (Some(mulf), Some(nf)) = (mul.as_f64(), n.as_f64()) {
                        if (nf / mulf).fract() != 0.0 {
                            add_error!("multipleOf", kind!(MultipleOf, n.clone(), mul.clone()));
                        }
                    }
                }
            }
            _ => {}
        }

        let validate_ref = |sch, kw: &'static str, uneval: &mut Uneval<'_>| {
            if let Err(ref_err) = validate_self(sch, kw.into(), uneval) {
                let mut err = error(
                    kw,
                    ErrorKind::Reference {
                        url: schemas.get(sch).loc.clone(),
                    },
                );
                if let ErrorKind::Group = ref_err.kind {
                    err.causes = ref_err.causes;
                } else {
                    err.causes.push(ref_err);
                }
                return Err(err);
            }
            Ok(())
        };

        // $ref --
        if let Some(ref_) = self.ref_ {
            add_err!(validate_ref(ref_, "$ref", uneval));
        }

        // $recursiveRef --
        if let Some(mut recursive_ref) = self.recursive_ref {
            if schemas.get(recursive_ref).recursive_anchor {
                let mut scope = &scope;
                loop {
                    let scope_sch = schemas.get(scope.sch);
                    let base_sch = schemas.get(scope_sch.resource);
                    if base_sch.recursive_anchor {
                        recursive_ref = scope.sch;
                    }
                    if let Some(parent) = scope.parent {
                        scope = parent;
                    } else {
                        break;
                    }
                }
            }
            add_err!(validate_ref(recursive_ref, "$recursiveRef", uneval));
        }

        // $dynamicRef --
        if let Some(mut dynamic_ref) = self.dynamic_ref {
            if let Some(dynamic_anchor) = &schemas.get(dynamic_ref).dynamic_anchor {
                let mut scope = &scope;
                loop {
                    let scope_sch = schemas.get(scope.sch);
                    let base_sch = schemas.get(scope_sch.resource);
                    debug_assert_eq!(base_sch.index, base_sch.resource);
                    if let Some(sch) = base_sch.dynamic_anchors.get(dynamic_anchor) {
                        dynamic_ref = *sch;
                    }
                    if let Some(parent) = scope.parent {
                        scope = parent;
                    } else {
                        break;
                    }
                }
            }
            add_err!(validate_ref(dynamic_ref, "$dynamicRef", uneval));
        }

        // not --
        if let Some(not) = self.not {
            if validate_self(not, "not".into(), uneval).is_ok() {
                add_error!("not", kind!(Not));
            }
        }

        // allOf --
        if !self.all_of.is_empty() {
            for (i, sch) in self.all_of.iter().enumerate() {
                let kw_path = format!("allOf/{i}");
                add_err!(validate_self(*sch, kw_path.into(), uneval));
            }
        }

        // anyOf --
        if !self.any_of.is_empty() {
            // NOTE: all schemas must be checked
            let mut anyof_errors = vec![];
            for (i, sch) in self.any_of.iter().enumerate() {
                let kw_path = format!("anyOf/{i}");
                if let Err(e) = validate_self(*sch, kw_path.into(), uneval) {
                    anyof_errors.push(e);
                }
            }
            if anyof_errors.len() == self.any_of.len() {
                // none matched
                errors.extend(anyof_errors);
            }
        }

        // oneOf --
        if !self.one_of.is_empty() {
            let (mut matched, mut oneof_errors) = (vec![], vec![]);
            for (i, sch) in self.one_of.iter().enumerate() {
                let kw_path = format!("oneOf/{i}");
                if let Err(e) = validate_self(*sch, kw_path.into(), uneval) {
                    oneof_errors.push(e);
                } else {
                    matched.push(i);
                    if matched.len() == 2 {
                        break;
                    }
                }
            }
            if matched.is_empty() {
                errors.extend(oneof_errors);
            } else if matched.len() > 1 {
                add_error!("oneOf", kind!(OneOf, got: matched));
            }
        }

        // if, then, else --
        if let Some(if_) = self.if_ {
            if validate_self(if_, "if".into(), uneval).is_ok() {
                if let Some(then) = self.then {
                    add_err!(validate_self(then, "then".into(), uneval));
                }
            } else if let Some(else_) = self.else_ {
                add_err!(validate_self(else_, "else".into(), uneval));
            }
        }

        // unevaluatedProps --
        if let (Some(sch), Value::Object(obj)) = (self.unevaluated_properties, v) {
            for pname in &uneval.props {
                if let Some(pvalue) = obj.get(*pname) {
                    let kw_path = "unevaluatedProperties";
                    add_err!(validate(sch, kw_path.into(), pvalue, &escape(pname)));
                }
            }
            uneval.props.clear();
        }

        // unevaluatedItems --
        if let (Some(sch), Value::Array(arr)) = (self.unevaluated_items, v) {
            for i in &uneval.items {
                if let Some(pvalue) = arr.get(*i) {
                    let kw_path = "unevaluatedItems";
                    add_err!(validate(sch, kw_path.into(), pvalue, &i.to_string()));
                }
            }
            uneval.items.clear();
        }

        match errors.len() {
            0 => Ok(_uneval),
            1 => Err(errors.remove(0)),
            _ => {
                let mut e = error("", kind!(Group));
                e.causes = errors;
                Err(e)
            }
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Type {
    Null,
    Bool,
    Number,
    Integer,
    String,
    Array,
    Object,
}

impl Type {
    fn of(v: &Value) -> Self {
        match v {
            Value::Null => Type::Null,
            Value::Bool(_) => Type::Bool,
            Value::Number(_) => Type::Number,
            Value::String(_) => Type::String,
            Value::Array(_) => Type::Array,
            Value::Object(_) => Type::Object,
        }
    }
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "null" => Some(Self::Null),
            "boolean" => Some(Self::Bool),
            "number" => Some(Self::Number),
            "integer" => Some(Self::Integer),
            "string" => Some(Self::String),
            "array" => Some(Self::Array),
            "object" => Some(Self::Object),
            _ => None,
        }
    }

    fn primitive(v: &Value) -> bool {
        !matches!(Self::of(v), Self::Array | Self::Object)
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Null => write!(f, "null"),
            Type::Bool => write!(f, "boolean"),
            Type::Number => write!(f, "number"),
            Type::Integer => write!(f, "integer"),
            Type::String => write!(f, "string"),
            Type::Array => write!(f, "array"),
            Type::Object => write!(f, "object"),
        }
    }
}

#[derive(Debug)]
pub struct ValidationError {
    pub keyword_location: String,
    pub absolute_keyword_location: String,
    pub instance_location: String,
    pub kind: ErrorKind,
    pub causes: Vec<ValidationError>,
}

impl ValidationError {
    fn print_alternate<'a>(
        &'a self,
        f: &mut std::fmt::Formatter,
        inst_loc: &'a str,
        mut sch_loc: &'a str,
        indent: usize,
    ) -> std::fmt::Result {
        for _ in 0..indent {
            write!(f, "  ")?;
        }
        if let ErrorKind::Schema { .. } = &self.kind {
            self.kind.fmt(f)?;
        } else {
            let inst_ptr = Loc::locate(inst_loc, &self.instance_location);
            let sch_ptr = Loc::locate(sch_loc, &self.absolute_keyword_location);
            write!(f, "I[{inst_ptr}] S[{sch_ptr}] {}", self.kind)?;
            // NOTE: this code used to check relative path correctness
            // let (_, ptr) = split(&self.absolute_keyword_location);
            // write!(
            //     f,
            //     "[I{inst_ptr}] [I{}] [S{sch_ptr}] [S{}]{}",
            //     self.instance_location, ptr, self.kind
            // )?;
        }
        sch_loc = if let ErrorKind::Reference { url } = &self.kind {
            url
        } else {
            &self.absolute_keyword_location
        };
        for cause in &self.causes {
            writeln!(f)?;
            cause.print_alternate(f, &self.instance_location, sch_loc, indent + 1)?;
        }
        Ok(())
    }
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            // NOTE: only root validationError supports altername display
            if let ErrorKind::Schema { url } = &self.kind {
                return self.print_alternate(f, "", url, 0);
            }
        }
        write!(
            f,
            "jsonschema: instance#{} does not validate with {}: {}",
            &self.instance_location, &self.absolute_keyword_location, self.kind
        )
    }
}

#[derive(Debug)]
pub enum ErrorKind {
    Group,
    Schema { url: String },
    Reference { url: String },
    RefCycle,
    FalseSchema,
    Type { got: Type, want: Vec<Type> },
    Enum { got: Value, want: Vec<Value> },
    Const { got: Value, want: Value },
    Format { got: Value, want: String },
    MinProperties { got: usize, want: usize },
    MaxProperties { got: usize, want: usize },
    AdditionalProperties { got: Vec<String> },
    Required { want: Vec<String> },
    DependentRequired { got: String, want: Vec<String> },
    MinItems { got: usize, want: usize },
    MaxItems { got: usize, want: usize },
    Contains,
    MinContains { got: Vec<usize>, want: usize },
    MaxContains { got: Vec<usize>, want: usize },
    UniqueItems { got: [usize; 2] },
    AdditionalItems { got: usize },
    MinLength { got: usize, want: usize },
    MaxLength { got: usize, want: usize },
    Pattern { got: String, want: String },
    ContentEncoding { got: String, want: String },
    ContentMediaType { got: Vec<u8>, want: String },
    Minimum { got: Number, want: Number },
    Maximum { got: Number, want: Number },
    ExclusiveMinimum { got: Number, want: Number },
    ExclusiveMaximum { got: Number, want: Number },
    MultipleOf { got: Number, want: Number },
    Not,
    AllOf { got: Vec<usize> },
    AnyOf,
    OneOf { got: Vec<usize> },
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // todo: use single quote for strings
        match self {
            Self::Group => write!(f, ""),
            Self::Schema { url } => write!(f, "validation failed with {url}"),
            Self::Reference { url } => write!(f, "fails to validate with {url}"),
            Self::RefCycle => write!(f, "reference cycle detected"),
            Self::FalseSchema => write!(f, "false schema"),
            Self::Type { got, want } => {
                // todo: why join not working for Type struct ??
                let want = join_iter(want, ", ");
                write!(f, "want {want}, but got {got}",)
            }
            Self::Enum { want, .. } => {
                if want.iter().all(Type::primitive) {
                    if want.len() == 1 {
                        write!(f, "value must be {want:?}")
                    } else {
                        let want = join_iter(want.iter().map(|e| format!("{e:?}")), " or ");
                        write!(f, "value must be one of {want}")
                    }
                } else {
                    write!(f, "enum failed")
                }
            }
            Self::Const { want, .. } => {
                if Type::primitive(want) {
                    write!(f, "value must be {want}")
                } else {
                    write!(f, "const failed")
                }
            }
            Self::Format { got, want } => write!(f, "{got} is not valid {want}"),
            Self::MinProperties { got, want } => write!(
                f,
                "minimum {want} properties allowed, but got {got} properties"
            ),
            Self::MaxProperties { got, want } => write!(
                f,
                "maximum {want} properties allowed, but got {got} properties"
            ),
            Self::AdditionalProperties { got } => {
                write!(
                    f,
                    "additionalProperties {} not allowed",
                    join_iter(got.iter().map(quote), ", ")
                )
            }
            Self::Required { want } => write!(
                f,
                "missing properties {}",
                join_iter(want.iter().map(quote), ", ")
            ),
            Self::DependentRequired { got, want } => write!(
                f,
                "properties {} required, if {} property exists",
                join_iter(want.iter().map(quote), ", "),
                quote(got)
            ),
            Self::MinItems { got, want } => {
                write!(f, "minimum {want} items allowed, but got {got} items")
            }
            Self::MaxItems { got, want } => {
                write!(f, "maximum {want} items allowed, but got {got} items")
            }
            Self::MinContains { got, want } => {
                if got.is_empty() {
                    write!(
                        f,
                        "minimum {want} items allowed to match contains schema, but found none",
                    )
                } else {
                    write!(
                        f,
                        "minimum {want} items allowed to match contains schema, but found {} items at {}",
                        got.len(),
                        join_iter(got, ", ")
                    )
                }
            }
            Self::Contains => write!(f, "no items match contains schema"),
            Self::MaxContains { got, want } => {
                if got.is_empty() {
                    write!(
                        f,
                        "maximum {want} items allowed to match contains schema, but found none",
                    )
                } else {
                    write!(
                        f,
                        "maximum {want} items allowed to match contains schema, but found {} items at {}",
                        got.len(),
                        join_iter(got, ", ")
                    )
                }
            }
            Self::UniqueItems { got: [i, j] } => write!(f, "items at {i} and {j} are equal"),
            Self::AdditionalItems { got } => write!(f, "got {got} additionalItems"),
            Self::MinLength { got, want } => write!(f, "length must be >={want}, but got {got}"),
            Self::MaxLength { got, want } => write!(f, "length must be <={want}, but got {got}"),
            Self::Pattern { got, want } => {
                write!(f, "{} does not match pattern {}", quote(got), quote(want))
            }
            Self::ContentEncoding { want, .. } => write!(f, "value is not {} encoded", quote(want)),
            Self::ContentMediaType { want, .. } => {
                write!(f, "value is not of mediatype {}", quote(want))
            }
            Self::Minimum { got, want } => write!(f, "must be >={want}, but got {got}"),
            Self::Maximum { got, want } => write!(f, "must be <={want}, but got {got}"),
            Self::ExclusiveMinimum { got, want } => write!(f, "must be > {want} but got {got}"),
            Self::ExclusiveMaximum { got, want } => write!(f, "must be < {want} but got {got}"),
            Self::MultipleOf { got, want } => write!(f, "{got} is not multipleOf {want}"),
            Self::Not => write!(f, "not failed"),
            Self::AllOf { got } => write!(f, "invalid against subschemas {}", join_iter(got, ", ")),
            Self::AnyOf => write!(f, "anyOf failed"),
            Self::OneOf { got } => {
                if got.is_empty() {
                    write!(f, "oneOf failed")
                } else {
                    write!(
                        f,
                        "want valid against oneOf subschema, but valid against subschemas {}",
                        join_iter(got, " and "),
                    )
                }
            }
        }
    }
}

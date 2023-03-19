use std::fmt::{Display, Formatter};

use serde::{
    ser::{SerializeMap, SerializeSeq},
    Serialize,
};

use crate::{
    util::*, validator::SchemaToken, ErrorKind, InstanceLocation, KeywordLocation, ValidationError,
};

impl<'s, 'v> ValidationError<'s, 'v> {
    fn display_causes(&self, f: &mut Formatter, unwrap: bool, indent: usize) -> std::fmt::Result {
        for (i, cause) in self.causes.iter().enumerate() {
            if i != 0 {
                writeln!(f)?;
            };
            cause.display(f, unwrap, indent)?;
        }
        Ok(())
    }

    pub(crate) fn display(
        &self,
        f: &mut Formatter,
        mut unwrap: bool,
        indent: usize,
    ) -> std::fmt::Result {
        if let ErrorKind::Schema { .. } = &self.kind {
            debug_assert_eq!(indent, 0, "ErrorKind::Schema must have zero indent");
            write!(f, "jsonschema {}", self.kind)?;
            writeln!(f)?;
            return self.display_causes(f, unwrap, indent + 1);
        }

        // unwrap --
        let (s, frag) = split(&self.absolute_keyword_location);
        if let ErrorKind::Reference { url } = &self.kind {
            if self.causes.len() == 1
                && (!f.alternate() || {
                    let (u, _) = split(url);
                    u == s // ref within root
                })
            {
                return self.causes[0].display(f, unwrap, indent);
            }
        }
        let unwrap_causes = !matches!(self.kind, ErrorKind::AnyOf { .. } | ErrorKind::OneOf(_));
        if unwrap
            && !self.causes.is_empty()
            && !matches!(
                self.kind,
                ErrorKind::Schema { .. }
                    | ErrorKind::Reference { .. }
                    | ErrorKind::AnyOf { .. }
                    | ErrorKind::OneOf(_)
                    | ErrorKind::ContentSchema
            )
        {
            return self.display_causes(f, unwrap_causes, indent);
        }
        unwrap = unwrap_causes;

        // indent --
        if indent > 0 {
            for _ in 0..indent - 1 {
                write!(f, "  ")?;
            }
            write!(f, "- ")?;
        }

        // location --
        let inst = &self.instance_location;
        write!(f, "at {}", quote(&inst.to_string()))?;
        if f.alternate() {
            write!(f, " [S#{frag}]")?;
        }
        write!(f, ": ")?;

        // message --
        if f.alternate() {
            match &self.kind {
                ErrorKind::Reference { url } => {
                    let (u, frag) = split(url);
                    if u == s {
                        write!(f, "validation failed with S#{frag}")?;
                    } else {
                        write!(f, "{}", self.kind)?;
                    }
                }
                _ => write!(f, "{}", self.kind)?,
            }
        } else {
            match &self.kind {
                ErrorKind::Reference { .. } => {
                    let kw = self
                        .keyword_location
                        .0
                        .iter()
                        .last()
                        .unwrap_or(&SchemaToken::Keyword(""));
                    write!(f, "{kw} failed")?
                }
                _ => write!(f, "{}", self.kind)?,
            }
        }

        // causes --
        if !self.causes.is_empty() {
            writeln!(f)?;
        }
        self.display_causes(f, unwrap, indent + 1)
    }

    pub fn flag_output(&self) -> FlagOutput {
        FlagOutput { valid: false }
    }

    pub fn basic_output(&self) -> OutputUnit {
        fn flatten<'e, 's, 'v>(
            err: &'e ValidationError<'s, 'v>,
            mut in_ref: bool,
            tgt: &mut Vec<OutputUnit<'e, 's, 'v>>,
        ) {
            in_ref = in_ref || matches!(err.kind, ErrorKind::Reference { .. });
            let absolute_keyword_location = if in_ref {
                Some(err.absolute_keyword_location.as_str())
            } else {
                None
            };
            tgt.push(OutputUnit {
                valid: false,
                keyword_location: &err.keyword_location,
                absolute_keyword_location,
                instance_location: &err.instance_location,
                error: OutputError::Leaf(&err.kind),
            });
            for cause in &err.causes {
                flatten(cause, in_ref, tgt);
            }
        }
        let error = if self.causes.is_empty() {
            OutputError::Leaf(&self.kind)
        } else {
            let mut v = vec![];
            for cause in &self.causes {
                flatten(cause, false, &mut v);
            }
            OutputError::Branch(v)
        };
        OutputUnit {
            valid: false,
            keyword_location: &self.keyword_location,
            absolute_keyword_location: None,
            instance_location: &self.instance_location,
            error,
        }
    }

    pub fn detailed_output(&self) -> OutputUnit {
        fn output_unit<'e, 's, 'v>(
            err: &'e ValidationError<'s, 'v>,
            mut in_ref: bool,
        ) -> OutputUnit<'e, 's, 'v> {
            in_ref = in_ref || matches!(err.kind, ErrorKind::Reference { .. });

            // single cause
            if !matches!(err.kind, ErrorKind::ContentSchema) {
                if let [cause] = &err.causes[..] {
                    return output_unit(cause, in_ref);
                }
            }

            let error = if err.causes.is_empty() {
                OutputError::Leaf(&err.kind)
            } else {
                let mut v = vec![];
                for cause in &err.causes {
                    v.push(output_unit(cause, in_ref));
                }
                OutputError::Branch(v)
            };
            let absolute_keyword_location = if in_ref {
                Some(err.absolute_keyword_location.as_str())
            } else {
                None
            };
            OutputUnit {
                valid: false,
                keyword_location: &err.keyword_location,
                absolute_keyword_location,
                instance_location: &err.instance_location,
                error,
            }
        }
        output_unit(self, false)
    }
}

pub struct FlagOutput {
    pub valid: bool,
}

impl Serialize for FlagOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("valid", &self.valid)?;
        map.end()
    }
}

impl Display for FlagOutput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write_json_to_fmt(f, self)
    }
}

pub struct OutputUnit<'e, 's, 'v> {
    pub valid: bool,
    pub keyword_location: &'e KeywordLocation<'s>,
    pub absolute_keyword_location: Option<&'e str>,
    pub instance_location: &'e InstanceLocation<'v>,
    pub error: OutputError<'e, 's, 'v>,
}

impl<'e, 's, 'v> Serialize for OutputUnit<'e, 's, 'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let n = 4 + self.absolute_keyword_location.map_or(0, |_| 1);
        let mut map = serializer.serialize_map(Some(n))?;
        map.serialize_entry("valid", &self.valid)?;
        map.serialize_entry("keywordLocation", &self.keyword_location.to_string())?;
        if let Some(s) = &self.absolute_keyword_location {
            map.serialize_entry("absoluteKeywordLocation", s)?;
        }
        map.serialize_entry("instanceLocation", &self.instance_location.to_string())?;
        let pname = match self.error {
            OutputError::Leaf(_) => "error",
            OutputError::Branch(_) => "errors",
        };
        map.serialize_entry(pname, &self.error)?;
        map.end()
    }
}

impl<'e, 's, 'v> Display for OutputUnit<'e, 's, 'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write_json_to_fmt(f, self)
    }
}

pub enum OutputError<'e, 's, 'v> {
    Leaf(&'e ErrorKind<'s>),
    Branch(Vec<OutputUnit<'e, 's, 'v>>),
}

impl<'e, 's, 'v> Serialize for OutputError<'e, 's, 'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            OutputError::Leaf(kind) => serializer.serialize_str(&kind.to_string()),
            OutputError::Branch(units) => {
                let mut seq = serializer.serialize_seq(Some(units.len()))?;
                for unit in units {
                    seq.serialize_element(unit)?;
                }
                seq.end()
            }
        }
    }
}

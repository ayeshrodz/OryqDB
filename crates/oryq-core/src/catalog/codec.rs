//! Versioned canonical `OryqDoc` encoding for persistent catalog records (`W08` frozen v1 schema).

use oryq_document::{encode_document, DocumentView, FieldPath, Value, ValueView};
use oryq_index::{IndexDescriptor, IndexFieldSpec, IndexKind, SortOrder};
use oryq_types::{
    CapabilityId, CollectionId, ErrorCategory, ErrorEnvelope, FeatureBitmask, IndexId, PageId,
    RecordId, Result,
};

const MODULE_ID: &str = "core";

/// Frozen M3 catalog record schema version.
pub const CATALOG_SCHEMA_VERSION: i64 = 1;

/// Reserved catalog B+ tree key storing global monotonic ID counters.
pub const CATALOG_META_KEY: &str = "__oryq_meta__";

/// Global monotonic counters persisted in the catalog B+ tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogMeta {
    pub next_collection_id: u64,
    pub next_index_id: u64,
}

impl CatalogMeta {
    pub const fn initial() -> Self {
        Self {
            next_collection_id: 1,
            next_index_id: 1,
        }
    }
}

/// Versioned persistent descriptor for one collection in the catalog B+ tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub collection_id: CollectionId,
    pub name: String,
    pub data_root_page_id: PageId,
    pub next_record_id: RecordId,
    pub required_write_features: FeatureBitmask,
    pub indexes: Vec<IndexDescriptor>,
}

impl CatalogEntry {
    /// Returns the collection's mandatory primary `_id` index descriptor.
    pub fn primary_index(&self) -> Result<&IndexDescriptor> {
        self.indexes
            .iter()
            .find(|idx| idx.kind == IndexKind::Primary)
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "CatalogEntry::primary_index",
                    format!("collection '{}' missing primary _id index", self.name),
                )
            })
    }
}

pub fn encode_catalog_meta(meta: &CatalogMeta) -> Result<Vec<u8>> {
    let fields = vec![
        (
            "schema_version".into(),
            Value::Int64(CATALOG_SCHEMA_VERSION),
        ),
        (
            "next_collection_id".into(),
            Value::Int64(meta.next_collection_id as i64),
        ),
        (
            "next_index_id".into(),
            Value::Int64(meta.next_index_id as i64),
        ),
    ];
    let (doc, _) = encode_document(&fields, None)?;
    Ok(doc.as_bytes().to_vec())
}

pub fn decode_catalog_meta(bytes: &[u8]) -> Result<CatalogMeta> {
    let view = DocumentView::from_bytes(bytes)?;
    let ver = req_i64(&view, "schema_version")?;
    if ver != CATALOG_SCHEMA_VERSION {
        return Err(ErrorEnvelope::new(
            ErrorCategory::UnsupportedFeature,
            MODULE_ID,
            "catalog::decode_catalog_meta",
            format!("unsupported catalog meta schema_version {ver}"),
        ));
    }
    let next_collection_id = req_i64(&view, "next_collection_id")? as u64;
    let next_index_id = req_i64(&view, "next_index_id")? as u64;
    Ok(CatalogMeta {
        next_collection_id,
        next_index_id,
    })
}

pub fn encode_catalog_entry(entry: &CatalogEntry) -> Result<Vec<u8>> {
    let mut idx_values = Vec::with_capacity(entry.indexes.len());
    for idx in &entry.indexes {
        idx_values.push(encode_index_descriptor(idx));
    }

    let fields = vec![
        (
            "schema_version".into(),
            Value::Int64(CATALOG_SCHEMA_VERSION),
        ),
        (
            "collection_id".into(),
            Value::Int64(entry.collection_id.raw() as i64),
        ),
        ("name".into(), Value::String(entry.name.clone())),
        (
            "data_root_page_id".into(),
            Value::Int64(entry.data_root_page_id.raw() as i64),
        ),
        (
            "next_record_id".into(),
            Value::Int64(entry.next_record_id.raw() as i64),
        ),
        (
            "required_write_features".into(),
            Value::Int64(entry.required_write_features.raw() as i64),
        ),
        ("indexes".into(), Value::Array(idx_values)),
    ];
    let (doc, _) = encode_document(&fields, None)?;
    Ok(doc.as_bytes().to_vec())
}

pub fn decode_catalog_entry(bytes: &[u8]) -> Result<CatalogEntry> {
    let view = DocumentView::from_bytes(bytes)?;
    let ver = req_i64(&view, "schema_version")?;
    if ver != CATALOG_SCHEMA_VERSION {
        return Err(ErrorEnvelope::new(
            ErrorCategory::UnsupportedFeature,
            MODULE_ID,
            "catalog::decode_catalog_entry",
            format!("unsupported catalog schema_version {ver}"),
        ));
    }
    let collection_id = CollectionId::new(req_i64(&view, "collection_id")? as u64)?;
    let name = req_str(&view, "name")?.to_owned();
    let data_root_page_id = PageId::new(req_i64(&view, "data_root_page_id")? as u64);
    let next_record_id = RecordId::new(req_i64(&view, "next_record_id")? as u64)?;
    let required_write_features =
        FeatureBitmask::from_raw(req_i64(&view, "required_write_features")? as u64);

    let mut indexes = Vec::new();
    match view.get_field("indexes") {
        Some(ValueView::Array(arr)) => {
            for item in arr.iter() {
                match item {
                    ValueView::Document(sub) => indexes.push(decode_index_descriptor(&sub)?),
                    _ => {
                        return Err(corrupt(
                            "catalog::decode_catalog_entry",
                            "catalog index entry must be a document",
                        ));
                    }
                }
            }
        }
        _ => {
            return Err(corrupt(
                "catalog::decode_catalog_entry",
                "catalog entry missing 'indexes' array",
            ));
        }
    }

    Ok(CatalogEntry {
        collection_id,
        name,
        data_root_page_id,
        next_record_id,
        required_write_features,
        indexes,
    })
}

fn encode_index_descriptor(idx: &IndexDescriptor) -> Value {
    let kind_str = match &idx.kind {
        IndexKind::Primary => "primary".to_owned(),
        IndexKind::Secondary => "secondary".to_owned(),
        IndexKind::Compound => "compound".to_owned(),
        IndexKind::Pluggable(k) => format!("pluggable:{k}"),
    };
    let fields_arr: Vec<Value> = idx
        .fields
        .iter()
        .map(|f| {
            Value::Document(vec![
                ("path".into(), Value::String(f.path.as_str().to_owned())),
                (
                    "order".into(),
                    Value::Int64(match f.order {
                        SortOrder::Asc => 1,
                        SortOrder::Desc => -1,
                    }),
                ),
            ])
        })
        .collect();

    let mut doc = vec![
        ("index_id".into(), Value::Int64(idx.index_id.raw() as i64)),
        ("name".into(), Value::String(idx.name.clone())),
        ("kind".into(), Value::String(kind_str)),
        ("fields".into(), Value::Array(fields_arr)),
        ("unique".into(), Value::Bool(idx.unique)),
        ("sparse".into(), Value::Bool(idx.sparse)),
        (
            "root_page_id".into(),
            Value::Int64(idx.root_page_id.raw() as i64),
        ),
        (
            "provider_id".into(),
            Value::String(idx.provider_id.as_str().to_owned()),
        ),
        (
            "data_version".into(),
            Value::Int64(i64::from(idx.data_version)),
        ),
    ];
    if let Some(bit) = idx.required_feature_bit {
        doc.push(("required_feature_bit".into(), Value::Int64(i64::from(bit))));
    }
    if let Some(ref opts) = idx.options_json {
        doc.push(("options_json".into(), Value::String(opts.clone())));
    }
    Value::Document(doc)
}

fn decode_index_descriptor(view: &DocumentView<'_>) -> Result<IndexDescriptor> {
    let index_id = IndexId::new(req_i64(view, "index_id")? as u64)?;
    let name = req_str(view, "name")?.to_owned();
    let kind = match req_str(view, "kind")? {
        "primary" => IndexKind::Primary,
        "secondary" => IndexKind::Secondary,
        "compound" => IndexKind::Compound,
        other if other.starts_with("pluggable:") => {
            IndexKind::Pluggable(other["pluggable:".len()..].to_owned())
        }
        other => {
            return Err(corrupt(
                "catalog::decode_index_descriptor",
                format!("unknown index kind {other:?}"),
            ));
        }
    };
    let unique = req_bool(view, "unique")?;
    let sparse = req_bool(view, "sparse")?;
    let root_page_id = PageId::new(req_i64(view, "root_page_id")? as u64);
    let provider_id = CapabilityId::new(req_str(view, "provider_id")?)?;
    let data_version = u16::try_from(req_i64(view, "data_version")?).map_err(|_| {
        corrupt(
            "catalog::decode_index_descriptor",
            "data_version out of u16 range",
        )
    })?;
    let required_feature_bit = match view.get_field("required_feature_bit") {
        Some(ValueView::Int64(v)) => Some(u8::try_from(v).map_err(|_| {
            corrupt(
                "catalog::decode_index_descriptor",
                "required_feature_bit out of u8 range",
            )
        })?),
        None => None,
        _ => {
            return Err(corrupt(
                "catalog::decode_index_descriptor",
                "required_feature_bit must be Int64",
            ));
        }
    };
    let options_json = match view.get_field("options_json") {
        Some(ValueView::String(s)) => Some(s.to_owned()),
        None => None,
        _ => {
            return Err(corrupt(
                "catalog::decode_index_descriptor",
                "options_json must be String",
            ));
        }
    };

    let mut fields = Vec::new();
    match view.get_field("fields") {
        Some(ValueView::Array(arr)) => {
            for item in arr.iter() {
                match item {
                    ValueView::Document(fdoc) => {
                        let path = FieldPath::parse(req_str(&fdoc, "path")?)?;
                        let order = match req_i64(&fdoc, "order")? {
                            1 => SortOrder::Asc,
                            -1 => SortOrder::Desc,
                            other => {
                                return Err(corrupt(
                                    "catalog::decode_index_descriptor",
                                    format!("invalid sort order {other}"),
                                ));
                            }
                        };
                        fields.push(IndexFieldSpec { path, order });
                    }
                    _ => {
                        return Err(corrupt(
                            "catalog::decode_index_descriptor",
                            "index field spec must be a document",
                        ));
                    }
                }
            }
        }
        _ => {
            return Err(corrupt(
                "catalog::decode_index_descriptor",
                "index descriptor missing 'fields' array",
            ));
        }
    }

    Ok(IndexDescriptor {
        index_id,
        name,
        kind,
        fields,
        unique,
        sparse,
        root_page_id,
        required_feature_bit,
        provider_id,
        data_version,
        options_json,
    })
}

fn req_i64(view: &DocumentView<'_>, name: &str) -> Result<i64> {
    match view.get_field(name) {
        Some(ValueView::Int64(v)) => Ok(v),
        _ => Err(corrupt(
            "catalog::codec",
            format!("catalog record missing Int64 field '{name}'"),
        )),
    }
}

fn req_str<'a>(view: &DocumentView<'a>, name: &str) -> Result<&'a str> {
    match view.get_field(name) {
        Some(ValueView::String(s)) => Ok(s),
        _ => Err(corrupt(
            "catalog::codec",
            format!("catalog record missing String field '{name}'"),
        )),
    }
}

fn req_bool(view: &DocumentView<'_>, name: &str) -> Result<bool> {
    match view.get_field(name) {
        Some(ValueView::Bool(b)) => Ok(b),
        _ => Err(corrupt(
            "catalog::codec",
            format!("catalog record missing Bool field '{name}'"),
        )),
    }
}

fn corrupt(op: &'static str, msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCategory::Corruption, MODULE_ID, op, msg)
}

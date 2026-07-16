use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::ModelPreset;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const METADATA_KEY: &str = "_devo";
const BUILTIN_SHA256_KEY: &str = "builtin_sha256";
const UPDATE_POLICY_KEY: &str = "update_policy";
const MANAGED_POLICY: &str = "managed";
const PINNED_POLICY: &str = "pinned";

#[derive(Debug)]
pub(super) struct UserCatalogSyncOutcome {
    pub(super) presets: Option<Vec<ModelPreset>>,
    pub(super) warnings: Vec<String>,
}

pub(super) fn synchronize_user_catalog(
    user_path: &Path,
    builtin_entries: &[Value],
) -> UserCatalogSyncOutcome {
    synchronize_user_catalog_with_writer(user_path, builtin_entries, write_atomic)
}

fn synchronize_user_catalog_with_writer<F>(
    user_path: &Path,
    builtin_entries: &[Value],
    writer: F,
) -> UserCatalogSyncOutcome
where
    F: FnOnce(&Path, &[u8]) -> std::io::Result<()>,
{
    let (mut user_entries, existed) = match read_user_entries(user_path) {
        Ok(entries) => entries,
        Err(message) => {
            return UserCatalogSyncOutcome {
                presets: None,
                warnings: vec![message],
            };
        }
    };

    if let Err(error) = parse_presets(&user_entries) {
        return UserCatalogSyncOutcome {
            presets: None,
            warnings: vec![format!("failed to parse model catalog: {error}")],
        };
    }

    let original_entries = user_entries.clone();
    synchronize_entries(&mut user_entries, builtin_entries);

    let presets = match parse_presets(&user_entries) {
        Ok(presets) => presets,
        Err(error) => {
            return UserCatalogSyncOutcome {
                presets: None,
                warnings: vec![format!(
                    "failed to parse synchronized model catalog: {error}"
                )],
            };
        }
    };

    let mut warnings = Vec::new();
    if !existed || user_entries != original_entries {
        match serde_json::to_string_pretty(&user_entries) {
            Ok(mut serialized) => {
                serialized.push('\n');
                if let Err(error) = writer(user_path, serialized.as_bytes()) {
                    warnings.push(format!(
                        "failed to persist synchronized model catalog: {error}"
                    ));
                }
            }
            Err(error) => warnings.push(format!(
                "failed to serialize synchronized model catalog: {error}"
            )),
        }
    }

    UserCatalogSyncOutcome {
        presets: Some(presets),
        warnings,
    }
}

fn read_user_entries(path: &Path) -> Result<(Vec<Value>, bool), String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), false));
        }
        Err(error) => return Err(format!("failed to read model catalog: {error}")),
    };

    if contents.trim().is_empty() {
        return Ok((Vec::new(), true));
    }

    serde_json::from_str(&contents)
        .map(|entries| (entries, true))
        .map_err(|error| format!("failed to parse model catalog: {error}"))
}

fn parse_presets(entries: &[Value]) -> Result<Vec<ModelPreset>, serde_json::Error> {
    serde_json::from_value(Value::Array(entries.to_vec()))
}

fn synchronize_entries(user_entries: &mut Vec<Value>, builtin_entries: &[Value]) {
    let builtins_by_slug: HashMap<&str, (&Value, String)> = builtin_entries
        .iter()
        .filter_map(|entry| {
            let slug = entry.get("slug")?.as_str()?;
            Some((slug, (entry, fingerprint(entry))))
        })
        .collect();
    let mut existing_slugs = HashSet::new();

    for user_entry in user_entries.iter_mut() {
        let Some(slug) = user_entry
            .get("slug")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        existing_slugs.insert(slug.clone());

        let Some((builtin_entry, builtin_hash)) = builtins_by_slug.get(slug.as_str()) else {
            continue;
        };

        match management_state(user_entry) {
            EntryManagement::Pinned => {}
            EntryManagement::Managed { source_hash } => {
                let user_hash = fingerprint(user_entry);
                if user_hash == source_hash {
                    *user_entry =
                        managed_builtin_entry(builtin_entry, Some(user_entry), builtin_hash);
                }
            }
            EntryManagement::Legacy => {
                if fingerprint(user_entry) == *builtin_hash {
                    *user_entry =
                        managed_builtin_entry(builtin_entry, Some(user_entry), builtin_hash);
                } else {
                    pin_entry(user_entry);
                }
            }
        }
    }

    for builtin_entry in builtin_entries {
        let Some(slug) = builtin_entry.get("slug").and_then(Value::as_str) else {
            continue;
        };
        if existing_slugs.insert(slug.to_owned()) {
            let builtin_hash = fingerprint(builtin_entry);
            user_entries.push(managed_builtin_entry(
                builtin_entry,
                /*previous_entry*/ None,
                &builtin_hash,
            ));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryManagement {
    Legacy,
    Managed { source_hash: String },
    Pinned,
}

fn management_state(entry: &Value) -> EntryManagement {
    let Some(metadata) = entry.get(METADATA_KEY).and_then(Value::as_object) else {
        return EntryManagement::Legacy;
    };
    let policy = metadata.get(UPDATE_POLICY_KEY).and_then(Value::as_str);
    let source_hash = metadata
        .get(BUILTIN_SHA256_KEY)
        .and_then(Value::as_str)
        .filter(|hash| is_valid_builtin_hash(hash))
        .map(str::to_owned);

    match (policy, source_hash) {
        (Some(PINNED_POLICY), _) => EntryManagement::Pinned,
        (Some(MANAGED_POLICY), Some(source_hash)) => EntryManagement::Managed { source_hash },
        (Some(MANAGED_POLICY), None) => EntryManagement::Legacy,
        (Some(_), _) => EntryManagement::Pinned,
        (None, Some(source_hash)) => EntryManagement::Managed { source_hash },
        (None, None) => EntryManagement::Legacy,
    }
}

fn is_valid_builtin_hash(hash: &str) -> bool {
    hash.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn fingerprint(entry: &Value) -> String {
    let mut content = entry.clone();
    if let Some(object) = content.as_object_mut() {
        object.remove(METADATA_KEY);
    }
    let canonical = canonicalize_json(&content);
    let digest = Sha256::digest(
        serde_json::to_vec(&canonical).expect("serializing serde_json::Value cannot fail"),
    );
    format!("sha256:{digest:x}")
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        Value::Object(object) => {
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                    .collect(),
            )
        }
        Value::Number(number) => Value::Number(canonical_number(number)),
        scalar => scalar.clone(),
    }
}

fn canonical_number(number: &serde_json::Number) -> serde_json::Number {
    if let Some(value) = number.as_i64() {
        return value.into();
    }
    if let Some(value) = number.as_u64() {
        return value.into();
    }

    let value = number
        .as_f64()
        .expect("serde_json numbers are finite numeric values");
    if value == 0.0 {
        return 0.into();
    }
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        return (value as i64).into();
    }
    serde_json::Number::from_f64(value).expect("serde_json numbers are finite")
}

fn managed_builtin_entry(
    builtin_entry: &Value,
    previous_entry: Option<&Value>,
    builtin_hash: &str,
) -> Value {
    let mut managed = builtin_entry.clone();
    let object = managed
        .as_object_mut()
        .expect("built-in model entries must be JSON objects");
    object.remove(METADATA_KEY);

    let mut metadata = previous_entry
        .and_then(|entry| entry.get(METADATA_KEY))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    metadata.insert(
        BUILTIN_SHA256_KEY.into(),
        Value::String(builtin_hash.to_owned()),
    );
    metadata.insert(
        UPDATE_POLICY_KEY.into(),
        Value::String(MANAGED_POLICY.into()),
    );
    object.insert(METADATA_KEY.into(), Value::Object(metadata));
    managed
}

fn pin_entry(entry: &mut Value) {
    let object = entry
        .as_object_mut()
        .expect("validated model entries must be JSON objects");
    let metadata = object
        .entry(METADATA_KEY)
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    metadata.as_object_mut().expect("metadata object").insert(
        UPDATE_POLICY_KEY.into(),
        Value::String(PINNED_POLICY.into()),
    );
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(data)?;
    temp_file.as_file().sync_all()?;
    temp_file
        .persist(path)
        .map(|_persisted_file| ())
        .map_err(|error| error.error)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::ModelPreset;
    use pretty_assertions::assert_eq;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::{synchronize_user_catalog, synchronize_user_catalog_with_writer};

    fn model(slug: &str, display_name: &str) -> Value {
        json!({
            "slug": slug,
            "display_name": display_name,
            "provider": "openai_chat_completions",
            "context_window": 200000,
            "input_modalities": ["text"]
        })
    }

    fn sha256(entry: &Value) -> String {
        let mut canonical = entry.clone();
        canonical
            .as_object_mut()
            .expect("model entry object")
            .remove("_devo");
        let digest = Sha256::digest(serde_json::to_vec(&canonical).expect("serialize model"));
        format!("sha256:{digest:x}")
    }

    fn managed(entry: &Value) -> Value {
        let mut expected = entry.clone();
        expected
            .as_object_mut()
            .expect("model entry object")
            .insert(
                "_devo".into(),
                json!({
                    "builtin_sha256": sha256(entry),
                    "update_policy": "managed"
                }),
            );
        expected
    }

    fn pinned(entry: &Value) -> Value {
        let mut expected = entry.clone();
        expected
            .as_object_mut()
            .expect("model entry object")
            .insert(
                "_devo".into(),
                json!({
                    "update_policy": "pinned"
                }),
            );
        expected
    }

    fn read_entries(path: &Path) -> Vec<Value> {
        serde_json::from_str(&fs::read_to_string(path).expect("read catalog"))
            .expect("parse catalog")
    }

    fn write_entries(path: &Path, entries: &[Value]) {
        fs::write(
            path,
            serde_json::to_string_pretty(entries).expect("serialize catalog"),
        )
        .expect("write catalog");
    }

    #[test]
    fn missing_user_catalog_creates_managed_builtin_copy() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("nested").join("models.json");
        let builtins = vec![model("a", "Builtin A"), model("b", "Builtin B")];

        let outcome = synchronize_user_catalog(&path, &builtins);

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert!(outcome.presets.is_some());
        assert_eq!(
            read_entries(&path),
            builtins.iter().map(managed).collect::<Vec<_>>()
        );
    }

    #[test]
    fn untouched_managed_entry_tracks_changed_builtin() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let previous = model("a", "Previous");
        write_entries(&path, &[managed(&previous)]);
        let current = model("a", "Current");

        let outcome = synchronize_user_catalog(&path, std::slice::from_ref(&current));

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![managed(&current)]);
    }

    #[test]
    fn edited_managed_entry_is_preserved() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let previous = model("a", "Previous");
        let mut edited = managed(&previous);
        edited["display_name"] = json!("User edit");
        write_entries(&path, std::slice::from_ref(&edited));
        let current = model("a", "Current");

        let outcome = synchronize_user_catalog(&path, &[current]);

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![edited]);
    }

    #[test]
    fn explicitly_pinned_entry_is_preserved() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let user = pinned(&model("a", "Pinned"));
        write_entries(&path, std::slice::from_ref(&user));

        let outcome = synchronize_user_catalog(&path, &[model("a", "Current")]);

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![user]);
    }

    #[test]
    fn invalid_managed_fingerprints_are_migrated_conservatively() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let mut missing = model("a", "Missing hash user edit");
        missing["_devo"] = json!({"update_policy": "managed"});
        let mut non_string = model("a", "Non-string hash user edit");
        non_string["_devo"] = json!({
            "builtin_sha256": 42,
            "update_policy": "managed"
        });
        let mut malformed = model("a", "Malformed hash user edit");
        malformed["_devo"] = json!({
            "builtin_sha256": "sha256:not-a-valid-digest",
            "update_policy": "managed"
        });
        write_entries(
            &path,
            &[missing.clone(), non_string.clone(), malformed.clone()],
        );
        missing["_devo"]["update_policy"] = json!("pinned");
        non_string["_devo"]["update_policy"] = json!("pinned");
        malformed["_devo"]["update_policy"] = json!("pinned");

        let outcome = synchronize_user_catalog(&path, &[model("a", "Builtin")]);

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![missing, non_string, malformed]);
    }

    #[test]
    fn legacy_entries_are_classified_without_losing_custom_models() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let unchanged = model("a", "Builtin A");
        let changed = model("b", "User B");
        let custom = model("custom", "Custom");
        write_entries(&path, &[unchanged.clone(), changed.clone(), custom.clone()]);
        let builtins = vec![unchanged.clone(), model("b", "Builtin B")];

        let outcome = synchronize_user_catalog(&path, &builtins);

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(
            read_entries(&path),
            vec![managed(&unchanged), pinned(&changed), custom]
        );
    }

    #[test]
    fn new_builtins_are_appended_and_removed_builtins_are_preserved() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let removed = managed(&model("removed", "Removed"));
        write_entries(&path, std::slice::from_ref(&removed));
        let added = model("added", "Added");

        let outcome = synchronize_user_catalog(&path, std::slice::from_ref(&added));

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![removed, managed(&added)]);
    }

    #[test]
    fn json_formatting_and_key_order_do_not_count_as_edits() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let mut previous = model("a", "Previous");
        previous["nested"] = json!({"alpha": 1, "beta": 2});
        let previous_hash = sha256(&previous);
        fs::write(
            &path,
            format!(
                r#"[{{"provider":"openai_chat_completions","slug":"a","nested":{{"beta":2,"alpha":1}},"input_modalities":["text"],"context_window":200000,"display_name":"Previous","_devo":{{"update_policy":"managed","builtin_sha256":"{previous_hash}"}}}}]"#
            ),
        )
        .expect("write reordered catalog");
        let current = model("a", "Current");

        let outcome = synchronize_user_catalog(&path, std::slice::from_ref(&current));

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![managed(&current)]);
    }

    #[test]
    fn equivalent_number_representations_do_not_count_as_edits() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let mut previous = model("a", "Previous");
        previous["temperature"] = json!(1);
        let mut user_entry = managed(&previous);
        user_entry["temperature"] = json!(1.0);
        write_entries(&path, &[user_entry]);
        let mut current = previous;
        current["display_name"] = json!("Current");

        let outcome = synchronize_user_catalog(&path, std::slice::from_ref(&current));

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![managed(&current)]);
    }

    #[test]
    fn unchanged_managed_catalog_is_not_rewritten() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let current = model("a", "Current");
        let managed_current = managed(&current);
        let compact =
            serde_json::to_string(&vec![managed_current]).expect("serialize compact JSON");
        fs::write(&path, &compact).expect("write compact catalog");

        let outcome = synchronize_user_catalog(&path, std::slice::from_ref(&current));

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(fs::read_to_string(&path).expect("read catalog"), compact);
    }

    #[test]
    fn managed_updates_preserve_unknown_metadata_fields() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let previous = model("a", "Previous");
        let mut managed_previous = managed(&previous);
        managed_previous["_devo"]["future_metadata"] = json!({"keep": true});
        write_entries(&path, &[managed_previous]);
        let current = model("a", "Current");
        let mut expected = managed(&current);
        expected["_devo"]["future_metadata"] = json!({"keep": true});

        let outcome = synchronize_user_catalog(&path, std::slice::from_ref(&current));

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![expected]);
    }

    #[test]
    fn unknown_model_fields_count_as_user_customization() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let previous = model("a", "Previous");
        let mut customized = managed(&previous);
        customized["future_provider_option"] = json!({"enabled": true});
        write_entries(&path, std::slice::from_ref(&customized));

        let outcome = synchronize_user_catalog(&path, &[model("a", "Current")]);

        assert_eq!(outcome.warnings, Vec::<String>::new());
        assert_eq!(read_entries(&path), vec![customized]);
    }

    #[test]
    fn invalid_user_json_is_not_overwritten() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let invalid = "{not valid json";
        fs::write(&path, invalid).expect("write invalid catalog");

        let outcome = synchronize_user_catalog(&path, &[model("a", "Builtin")]);

        assert_eq!(outcome.presets, None);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("failed to parse model catalog"));
        assert_eq!(
            fs::read_to_string(&path).expect("read invalid catalog"),
            invalid
        );
    }

    #[test]
    fn write_failure_returns_synchronized_presets_and_warning() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("models.json");
        let previous = model("a", "Previous");
        write_entries(&path, &[managed(&previous)]);
        let current = model("a", "Current");

        let outcome = synchronize_user_catalog_with_writer(
            &path,
            std::slice::from_ref(&current),
            |_path, _data| Err(std::io::Error::other("injected write failure")),
        );

        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("failed to persist synchronized model catalog"));
        assert_eq!(read_entries(&path), vec![managed(&previous)]);
        let expected_presets: Vec<ModelPreset> =
            serde_json::from_value(Value::Array(vec![current])).expect("parse current preset");
        assert_eq!(outcome.presets, Some(expected_presets));
    }
}

use serde::{Deserialize, Deserializer};

// TODO: Use trait to allow only either `only` or `not`
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnlyOrNot {
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub only: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub not: Option<Vec<String>>,
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        String(String),
        Vec(Vec<String>),
    }

    let vec = match StringOrVec::deserialize(deserializer)? {
        StringOrVec::Vec(vec) => vec,
        StringOrVec::String(string) => vec![string],
    };
    Ok(Some(vec))
}

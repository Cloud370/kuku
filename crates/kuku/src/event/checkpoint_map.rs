use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) fn serialize<'a, K, V, M, S>(map: &'a M, serializer: S) -> Result<S::Ok, S::Error>
where
    K: Serialize + 'a,
    V: Serialize + 'a,
    &'a M: IntoIterator<Item = (&'a K, &'a V)>,
    S: Serializer,
{
    serializer.collect_seq(map)
}

pub(crate) fn deserialize<'de, K, V, M, D>(deserializer: D) -> Result<M, D::Error>
where
    K: Deserialize<'de>,
    V: Deserialize<'de>,
    M: FromIterator<(K, V)>,
    D: Deserializer<'de>,
{
    Vec::<(K, V)>::deserialize(deserializer).map(|entries| entries.into_iter().collect())
}

/// A decoded, not yet validated boss catalog; entries keep file order.
///
/// Unknown keys, non-string values and explicit nulls are the loader's to reject;
/// `None` here always means "key absent".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogSpec {
    pub difficulties: Vec<DifficultySpec>,
    pub bosses: Vec<BossSpec>,
}

/// One `difficulties:` entry, e.g. `n: Normal`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DifficultySpec {
    pub prefix: String,
    pub label: String,
}

/// One `bosses:` entry keyed by its short name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BossSpec {
    pub short: String,
    /// Defaults to `short`.
    pub full: Option<String>,
    pub level: Option<i64>,
    /// Defaults to every catalog difficulty.
    pub difficulties: Option<Vec<String>>,
    pub aliases: Vec<String>,
    pub portrait: Option<String>,
    pub guide: Option<GuideSpec>,
}

/// The optional `guide:` map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuideSpec {
    /// Required when `guide:` is present.
    pub colour: Option<i64>,
}

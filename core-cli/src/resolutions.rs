//! Shell query routing only; discovery and access decisions remain in Core.
use bili_core::{Core, MediaItem, Part, PartRef, QualityCatalog, Result};

pub(crate) trait CatalogSource {
    async fn resolve(&self, input: &str) -> Result<MediaItem>;
    async fn catalog(&self, part: PartRef) -> Result<QualityCatalog>;
}

impl CatalogSource for Core {
    async fn resolve(&self, input: &str) -> Result<MediaItem> {
        self.resolve(input.into()).await
    }
    async fn catalog(&self, part: PartRef) -> Result<QualityCatalog> {
        self.quality_catalog(part).await
    }
}

pub(crate) struct Report<'a> {
    pub index: usize,
    pub item: Option<&'a MediaItem>,
    pub part: Option<&'a Part>,
    pub result: Result<QualityCatalog>,
}

pub(crate) async fn query(
    source: &impl CatalogSource,
    inputs: &[String],
    parts: &str,
    mut emit: impl FnMut(Report<'_>) -> Result<()>,
) -> Result<u8> {
    let mut code = 0;
    for (position, input) in inputs.iter().enumerate() {
        let index = position + 1;
        let item = match source.resolve(input).await {
            Ok(item) => item,
            Err(error) => {
                code = code.max(crate::output::exit_code(&error));
                emit(Report {
                    index,
                    item: None,
                    part: None,
                    result: Err(error),
                })?;
                continue;
            }
        };
        let selected = match crate::selected_parts(&item.parts, parts) {
            Ok(parts) if !parts.is_empty() => parts,
            result => {
                let error = result
                    .err()
                    .unwrap_or_else(|| crate::invalid("part ordinal unavailable"));
                code = code.max(crate::output::exit_code(&error));
                emit(Report {
                    index,
                    item: Some(&item),
                    part: None,
                    result: Err(error),
                })?;
                continue;
            }
        };
        for part in selected {
            let result = source.catalog(part.reference.clone()).await;
            if let Err(error) = &result {
                code = code.max(crate::output::exit_code(error));
            }
            emit(Report {
                index,
                item: Some(&item),
                part: Some(part),
                result,
            })?;
        }
    }
    Ok(code)
}

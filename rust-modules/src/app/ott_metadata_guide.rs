//! Admission of typed guide replies into the existing selected-entry preview.
//! No work, time sample, or new publication authority is created here.
use plx_data::ott::{
    guide::{Query, Request, ResultSet},
    CatalogEntry, ProviderProfile,
};

pub(crate) struct Guide {
    pub row_guide: Option<super::super::ott_runtime::row_guide::Reply>,
    /// Exact effective source used by the worker, not rebound from current Saved.
    /// The publication owner/profile_id can represent an outer Favorite owner.
    pub source_profile: ProviderProfile,
    pub generation: u64,
    pub profile_id: String,
    /// The visible selection may be a favorite rather than the resolved entry.
    pub entry_id: String,
    pub entry: CatalogEntry,
    pub request: Request,
    pub result: Result<ResultSet, &'static str>,
}

impl Guide {
    pub(crate) fn is_current(
        &self, generation: u64, target: Option<&(String, String)>,
        query: Option<&Query>, cancelled: bool,
    ) -> bool {
        if cancelled
            || self.generation != generation
            || target.is_none_or(|(profile, selection)| {
                profile != &self.profile_id || selection != &self.entry_id
            })
            || query != Some(&self.request.query)
            || self.request.context.generation != self.generation
            || self.request.context.profile_id != self.profile_id
            || self.request.context.entry_id != self.entry.id
            || self.result.as_ref().is_ok_and(|result| !result.can_publish(&self.request, false))
        {
            return false;
        }
        true
    }


    pub(crate) fn into_preview(
        self,
        generation: u64,
        target: Option<&(String, String)>,
        query: Option<&Query>,
        cancelled: bool,
    ) -> Option<super::Preview> {
        if !self.is_current(generation, target, query, cancelled) { return None; }
        let result = match self.result {
            Ok(result) => {
                if !result.can_publish(&self.request, false) {
                    return None;
                }
                Ok(serde_json::json!({"programmes": result.programmes}))
            }
            Err(error) => Err(error),
        };
        Some(super::Preview {
            generation: self.generation,
            profile_id: self.profile_id,
            entry_id: self.entry_id,
            entry: Some(self.entry),
            result,
        })
    }
}

#[cfg(test)]
#[path = "ott_metadata_guide_tests.rs"]
mod tests;

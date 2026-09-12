//! Datasets, queries and recipes.

use super::*;

impl Host {
    /// Where a workspace's dataset list lives.
    ///
    /// Takes the workspace rather than reading it, because the one caller that
    /// matters — the registry rebuild — is already holding it and reading it
    /// again from inside the same lock would deadlock.
    pub(super) fn data_dir_for(&self, workspace: &Path) -> PathBuf {
        taurus_data::data_dir(
            &config::home_dir(),
            &crate::sessions::workspace_key(workspace),
        )
    }

    /// Every dataset loaded in this workspace, in the order they were loaded.
    pub async fn datasets(&self) -> Vec<taurus_data::Dataset> {
        let workspace = self.workspace().await;
        taurus_data::catalog::load(&self.data_dir_for(&workspace))
    }

    /// Reads a dataset in full and reports its shape.
    ///
    /// Computed on demand rather than cached with the entry. A profile is a
    /// statement about the file as it is now, and a stored one would be right
    /// until somebody rewrote the file and wrong silently afterwards — which is
    /// the failure a profile exists to catch, arriving from the tool meant to
    /// catch it.
    pub async fn dataset_profile(&self, name: &str) -> Result<taurus_data::Profile, String> {
        let (source, _) = self.dataset_source(name).await?;
        self.engine
            .profile(&source)
            .await
            .map_err(|e| e.to_string())
    }

    /// The columns of every dataset loaded here, without reading any of them.
    ///
    /// [`taurus_data::Engine::schema`] rather than `profile`, and that is the
    /// whole point: a profile is a full scan and this is asked for on every
    /// visit to the query box. A Parquet footer answers it instantly and a CSV
    /// costs the few rows the inference reads.
    ///
    /// A dataset whose file has gone is **left out rather than failing the
    /// call**. This exists to feed completion, and a workspace with one stale
    /// entry should still be able to complete the other three — the same
    /// argument recipes make for carrying their problems beside the list. The
    /// missing file is not silently swallowed either: opening that dataset in
    /// the pane says so, from the read that actually needed it.
    pub async fn dataset_schemas(&self) -> Vec<(taurus_data::Dataset, taurus_data::Schema)> {
        let workspace = self.workspace().await;
        // Asked for all at once rather than one after another: each is a
        // Parquet footer read or a CSV inference pass, and the query box asks
        // on every visit, so it waited for the sum of them rather than the
        // slowest. `join_all` answers in the order it was given, which is the
        // catalog's.
        let reads = self.datasets().await.into_iter().filter_map(|dataset| {
            // Through the guard, like every other read of an entry's path. An
            // entry is a line in a file somebody can edit, so `../` in one is a
            // thing that can happen rather than a thing that cannot.
            let path = taurus_tools::path_guard::resolve(&workspace, &dataset.path).ok()?;
            let source = taurus_data::Source::at(path).ok()?;
            Some(async move {
                let schema = self.engine.schema(&source).await.ok()?;
                Some((dataset, schema))
            })
        });
        futures::future::join_all(reads)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// A window of a dataset's rows.
    pub async fn dataset_page(
        &self,
        name: &str,
        offset: u64,
        limit: u64,
    ) -> Result<taurus_data::Page, String> {
        let (source, _) = self.dataset_source(name).await?;
        self.engine
            .page(&source, offset, limit)
            .await
            .map_err(|e| e.to_string())
    }

    /// Drops a dataset from the list, and returns what is left.
    ///
    /// The file is untouched. Returning the remainder rather than nothing so a
    /// pane can redraw from the answer, the same way [`Self::forget_note`]
    /// does.
    pub async fn forget_dataset(&self, name: &str) -> Result<Vec<taurus_data::Dataset>, String> {
        let workspace = self.workspace().await;
        let dir = self.data_dir_for(&workspace);
        taurus_data::catalog::forget(&dir, name).map_err(|e| e.to_string())?;
        Ok(taurus_data::catalog::load(&dir))
    }

    /// Answers one read-only query over every dataset loaded here.
    ///
    /// Read-only is enforced by the engine rather than by this, and it has to
    /// be: the pane hands over whatever was typed into a box, so the
    /// difference between a query and a `COPY … TO` is a refusal one layer
    /// down. See [`taurus_data::Engine::query`].
    pub async fn query_data(&self, sql: &str) -> Result<taurus_data::QueryResult, String> {
        let workspace = self.workspace().await;
        let tables = taurus_data::tables(&self.data_dir_for(&workspace), &workspace);
        if tables.is_empty() {
            return Err(
                "No datasets are loaded in this workspace, so there is nothing to query.".into(),
            );
        }
        self.engine
            .query(&tables, sql, taurus_data::MAX_QUERY_ROWS)
            .await
            .map_err(|e| match e {
                // The same courtesy the tool gets: a wrong table name is one
                // line from a right one.
                taurus_data::DataError::BadQuery { .. } => {
                    let names: Vec<&str> = tables.iter().map(|(n, _)| n.as_str()).collect();
                    format!("{e} Tables here: {}.", names.join(", "))
                }
                other => other.to_string(),
            })
    }

    /// Every recipe this workspace has, and anything wrong with the rest.
    ///
    /// Problems travel beside the list rather than instead of it, the same way
    /// a skill's warnings do: one torn file should cost the reader that file
    /// and not the other four.
    pub async fn recipes(&self) -> (Vec<taurus_data::Recipe>, Vec<String>) {
        taurus_data::recipe::load(&self.workspace().await)
    }

    /// Runs a recipe and writes the file it names.
    ///
    /// The one method here that changes the workspace, and the only caller is
    /// somebody clicking Run on a button that says where it writes. That is
    /// the same arrangement [`Self::query_data`] has with the query box: the
    /// person is doing it, so there is nobody to ask — but unlike a query this
    /// leaves a file behind, so the button has to name it and the pane has to
    /// keep naming it.
    ///
    /// Read-only is still enforced per step, one layer down, for the reason it
    /// always is: the button promised one path.
    pub async fn run_recipe(&self, name: &str) -> Result<taurus_data::Materialized, String> {
        let workspace = self.workspace().await;
        let recipe = taurus_data::recipe::find(&workspace, name).map_err(|e| e.to_string())?;
        let loaded = taurus_data::tables(&self.data_dir_for(&workspace), &workspace);
        let (tables, start) =
            taurus_data::recipe::resolve(&recipe, &workspace, loaded).map_err(|e| e.to_string())?;
        let output = taurus_tools::path_guard::resolve(&workspace, &recipe.output)
            .map_err(|e| e.to_string())?;

        let steps: Vec<(String, String)> = recipe
            .steps
            .iter()
            .map(|step| (step.title.clone(), step.sql.clone()))
            .collect();
        let mut run = self
            .engine
            .materialize(&tables, &start, &steps, &output)
            .await
            .map_err(|e| e.to_string())?;

        // Loaded on the way out, so the pane can show what came out without a
        // second action. See `list_output`.
        run.unlisted = list_output(&self.data_dir_for(&workspace), &workspace, &output);
        Ok(run)
    }

    /// Resolves a named dataset to a file this workspace is allowed to read.
    ///
    /// Through the path guard rather than by joining, and that is not
    /// ceremony. The list is a JSON file in the config home: it is
    /// hand-editable, it survives a workspace being moved, and an entry whose
    /// path climbed out of the tree with `..` would otherwise have every
    /// command here read a file outside the folder the user opened.
    pub(super) async fn dataset_source(
        &self,
        name: &str,
    ) -> Result<(taurus_data::Source, taurus_data::Dataset), String> {
        let workspace = self.workspace().await;
        let dataset = taurus_data::catalog::find(&self.data_dir_for(&workspace), name)
            .map_err(|e| e.to_string())?;
        let path = taurus_tools::path_guard::resolve(&workspace, &dataset.path)
            .map_err(|e| e.to_string())?;
        let source = taurus_data::Source::at(path).map_err(|e| e.to_string())?;
        Ok((source, dataset))
    }
}

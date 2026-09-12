//! Datasets, queries and recipes.

use super::*;

#[tauri::command]
pub async fn list_datasets(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<Dataset>> {
    Ok(state.host.datasets().await)
}

/// Reads a dataset in full and describes every column.
///
/// The one command here that can take real time — it is a scan of the whole
/// file — which is why the pane shows a reading state over it rather than
/// waiting in silence.
#[tauri::command]
pub async fn dataset_profile(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> CmdResult<DataProfile> {
    state.host.dataset_profile(&name).await
}

/// One loaded dataset and its columns, without anything having been counted.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct DataTable {
    pub name: String,
    pub path: String,
    pub columns: Vec<taurus_data::ColumnHead>,
    /// Rows, where the format keeps a count. Parquet does; a CSV would have to
    /// be read, and this call is the one that refuses to read anything.
    #[ts(type = "number | null")]
    pub rows: Option<u64>,
}

/// Every table a query in this workspace can name, and what is in it.
///
/// The query box's own schema. Kept apart from `dataset_profile` because the
/// two questions are different sizes: a profile counts nulls and distinct
/// values over the whole file, and this reads a header. Completion needs the
/// second one every time the box is opened and could never afford the first.
///
/// A dataset whose file has gone is left out rather than failing the call —
/// see `Host::dataset_schemas`.
#[tauri::command]
pub async fn dataset_tables(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<DataTable>> {
    Ok(state
        .host
        .dataset_schemas()
        .await
        .into_iter()
        .map(|(dataset, schema)| DataTable {
            name: dataset.name,
            path: dataset.path,
            columns: schema.columns,
            rows: schema.rows,
        })
        .collect())
}

/// A window of a dataset's rows.
///
/// `limit` is clamped by the engine rather than trusted, so a frontend asking
/// for the whole file gets a page and not a hang. See `taurus_data::MAX_PAGE`.
#[tauri::command]
pub async fn dataset_page(
    state: State<'_, Arc<AppState>>,
    name: String,
    offset: u64,
    limit: u64,
) -> CmdResult<DataPage> {
    state.host.dataset_page(&name, offset, limit).await
}

/// Answers one read-only SQL question over every dataset loaded here.
///
/// The engine refuses anything that is not a read, which is what lets this be
/// a plain command rather than one behind a confirmation: the box in the pane
/// takes arbitrary text, and `COPY … TO` is one line of SQL.
#[tauri::command]
pub async fn query_data(
    state: State<'_, Arc<AppState>>,
    sql: String,
) -> CmdResult<DataQueryResult> {
    state.host.query_data(&sql).await
}

/// Every recipe this workspace has, with anything wrong with the rest.
///
/// Problems travel with the list rather than as a failure. A recipe that will
/// not parse is a file somebody is halfway through writing, and hiding the
/// other four while they finish would be the pane arguing with an editor.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct Recipes {
    pub recipes: Vec<Recipe>,
    /// One line per unreadable file, each naming its path.
    pub problems: Vec<String>,
}

#[tauri::command]
pub async fn list_recipes(state: State<'_, Arc<AppState>>) -> CmdResult<Recipes> {
    let (recipes, problems) = state.host.recipes().await;
    Ok(Recipes { recipes, problems })
}

/// Runs a recipe and writes the file it names.
///
/// The one command in this family that changes the workspace, and it does it
/// without a permission prompt for the same reason `query_data` does not have
/// one: the person clicked the button, and the button says what it writes.
/// What the engine still refuses is a *step* that writes somewhere else — the
/// button named one path and only that path was agreed to.
#[tauri::command]
pub async fn run_recipe(state: State<'_, Arc<AppState>>, name: String) -> CmdResult<DataRun> {
    let run = state.host.run_recipe(&name).await?;
    // A recipe's output is loaded as a dataset on the way out, so the pane's
    // list and the rail's count have both just changed.
    emit_status(&state).await;
    Ok(run)
}

/// Drops a dataset from the list and answers with what is left.
///
/// The file it pointed at is not touched. Forgetting a dataset is the opposite
/// of a destructive act — it is the way to correct a mistaken load — so it
/// arms no confirmation, unlike deleting a conversation.
#[tauri::command]
pub async fn forget_dataset(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> CmdResult<Vec<Dataset>> {
    let left = state.host.forget_dataset(&name).await?;
    // The tab disappears when the last one goes, so the rest of the window has
    // to hear about it.
    emit_status(&state).await;
    Ok(left)
}

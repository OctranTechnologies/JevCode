use super::ToolOutput;
use crate::{
    error::AppResult,
    git,
    workspaces::{list_directory, scan_project},
};
use serde_json::json;
use std::path::Path;

pub async fn inspect_project(root: &Path) -> AppResult<ToolOutput> {
    let scan = scan_project(root)?;
    let files = list_directory(root, ".")?;
    let status = git::status(root).await.ok();
    let branch = git::branch(root).await.ok().flatten();
    let project_name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Project".into());
    let file_names: Vec<_> = files
        .iter()
        .take(100)
        .map(|entry| json!({"name":entry.name,"kind":entry.kind,"path":entry.path}))
        .collect();
    let language_names: Vec<_> = scan
        .languages
        .iter()
        .map(|language| json!({"name":language.name,"files":language.files}))
        .collect();
    let data = json!({"project":project_name,"repositoryBranch":branch,"gitStatus":status,"repositorySizeBytes":scan.size_bytes,"scanLimited":scan.limited,"languages":language_names,"rootEntries":file_names});
    Ok(ToolOutput {
        content: data.to_string(),
        structured: data,
    })
}

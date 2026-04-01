use std::path::{
    Path,
    PathBuf,
};

use tokio::fs;

use super::{
    AppEntity,
    ArtifactError,
    templates,
};

pub async fn write_artifacts(entity: &AppEntity, deploy_dir: &Path) -> Result<(), ArtifactError> {
    write_artifact(
        deploy_dir.join("Containerfile"),
        templates::render_containerfile(&entity.config)?,
    )
    .await?;

    write_artifact(
        deploy_dir.join("run.sh"),
        templates::render_run_script(&entity.config.runtime)?,
    )
    .await?;

    for (index, layer) in entity.config.build.iter().enumerate() {
        let path = deploy_dir.join(format!("build-{}.sh", index + 1));
        write_artifact(path, templates::render_build_script(layer)?).await?;
    }

    Ok(())
}

async fn write_artifact(path: PathBuf, contents: String) -> Result<(), ArtifactError> {
    fs::write(&path, contents)
        .await
        .map_err(|source| ArtifactError::Write { path, source })
}

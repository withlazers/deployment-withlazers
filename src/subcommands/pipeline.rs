use std::ops::Deref;

use crate::result::Result;
use clap::Parser;
use git2::{BranchType, Commit, Oid, Reference, Repository, Submodule};
use log::{debug, info, trace};
use tempfile::{tempdir, TempDir};

#[derive(Parser, Debug, Clone)]
pub struct Args {
    #[arg(short, long, default_value = ".")]
    repository: String,
    #[arg(short, long)]
    composite_repository: String,
}

pub struct TempRepository {
    repository: Repository,
    // tempdir is never read, but must be kept as long as the Repository
    // is accessed.
    #[allow(dead_code)]
    tempdir: TempDir,
}
impl TempRepository {
    pub fn clone_recurse(url: &str) -> Result<Self> {
        let tempdir = tempdir()?;
        trace!("Cloning {} into {}", url, tempdir.path().display());
        let repository = Repository::clone_recurse(url, tempdir.path())?;
        Ok(Self {
            repository,
            tempdir,
        })
    }
}

impl Deref for TempRepository {
    type Target = Repository;
    fn deref(&self) -> &Self::Target {
        &self.repository
    }
}

fn checkout_composite(url: &str, branch: &Reference) -> Result<TempRepository> {
    let repo = TempRepository::clone_recurse(url).unwrap();

    let shorthand = branch.shorthand().unwrap();
    {
        let branch_commit = if let Ok(branch) = repo.find_branch(
            &format!("origin/{}", shorthand),
            git2::BranchType::Remote,
        ) {
            branch.get().peel_to_commit()?
        } else {
            repo.head()?.peel_to_commit()?
        };

        debug!("Creating branch {:?} in {:?}", shorthand, repo.path());
        let new_branch = repo.branch(shorthand, &branch_commit, false)?;
        debug!("Created branch {:?}", new_branch.name());
    }

    repo.set_head(&format!("refs/heads/{}", shorthand))?;
    repo.checkout_head(None)?;

    Ok(repo)
}

fn find_submodule<'a>(
    composite_repo: &'a Repository,
    head: &Reference,
) -> Result<Submodule<'a>> {
    let commit = head.peel_to_commit()?;
    let submodules = composite_repo.submodules()?;
    let (_, submodule) = submodules
        .into_iter()
        .map(|x| (x.open().unwrap(), x))
        .find(|(repository, _)| repository.find_commit(commit.id()).is_ok())
        .ok_or("No submodule found")?;
    info!("Found submodule: {:?}", submodule.path());
    Ok(submodule)
}

fn update_submodule(submodule: &mut Submodule, id: Oid) -> Result<()> {
    let sub_repository = submodule.open()?;
    let commit = sub_repository.find_commit(id)?;
    info!("Found commit: {:?}", commit);
    sub_repository.set_head_detached(commit.id())?;
    submodule.add_to_index(true)?;
    info!("Updated {:?}", sub_repository.path());
    Ok(())
}

fn push_composite(composite_repo: &TempRepository) -> Result<()> {
    let mut remote = composite_repo.find_remote("origin")?;
    let mut push_options = git2::PushOptions::new();
    let callbacks = git2::RemoteCallbacks::new();
    // TODO: Add authentication and error reporting
    push_options.remote_callbacks(callbacks);

    // https://docs.rs/git2/latest/git2/struct.RemoteCallbacks.html
    // git -c http.https://<url of submodule repository>.extraheader="AUTHORIZATION: basic <BASE64_ENCODED_TOKEN_DESCRIBED_ABOVE>" submodule update --init --recursive
    // https://learn.microsoft.com/en-us/azure/devops/pipelines/repos/pipeline-options-for-git?view=azure-devops&tabs=yaml#alternative-to-using-the-checkout-submodules-option
    let head = composite_repo.head()?;
    if !head.is_branch() {
        return Err("Composite repository is not on a branch".into());
    }

    remote.push(&[head.name().unwrap()], Some(&mut push_options))?;
    Ok(())
}

fn commit_composite(
    composite_repo: &TempRepository,
    submodule: &Submodule,
    original_commit: &Commit,
) -> Result<()> {
    let mut index = composite_repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = composite_repo.find_tree(tree_id)?;
    let head = composite_repo.head()?.peel_to_commit()?;
    let commit = composite_repo.find_commit(head.id())?;
    let message = format!(
        "Update submodule {} to {}\n---\n{}",
        submodule.path().display(),
        commit.id(),
        original_commit.message().unwrap()
    );
    composite_repo.commit(
        Some("HEAD"),
        &original_commit.author(),
        &original_commit.committer(),
        &message,
        &tree,
        &[&commit],
    )?;
    Ok(())
}

pub fn run(args: Args) -> Result<()> {
    let repo = Repository::open(args.repository)?;

    let head = repo.head()?;
    if !head.is_branch() {
        return Err("HEAD is not a branch".into());
    }

    let composite_repo = checkout_composite(&args.composite_repository, &head)?;

    let mut submodule = find_submodule(&composite_repo, &head)?;

    let commit = head.peel_to_commit()?;

    update_submodule(&mut submodule, commit.id())?;

    commit_composite(&composite_repo, &submodule, &commit)?;

    push_composite(&composite_repo)?;

    Ok(())
}

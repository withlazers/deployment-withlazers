use std::ops::Deref;

use crate::result::Result;
use clap::Parser;
use git2::{Commit, FetchOptions, Oid, PushOptions, Reference, Repository, Submodule};
use git2::build::RepoBuilder;
use log::{debug, info, trace};
use tempfile::{tempdir, TempDir};

#[derive(Parser, Debug, Clone)]
pub struct Args {
    /// Repository that is updated
    #[arg(short, long, default_value = ".")]
    repository: String,

    /// The composite repository
    #[arg(short, long)]
    composite_repository: String,

    /// Set custom headers for pulling and pushing
    #[arg(short, long)]
    custom_headers: Vec<String>,
}

pub struct TempRepository {
    repository: Repository,
    // temp_dir is never read, but must be kept as long as the Repository
    // is accessed.
    #[allow(dead_code)]
    temp_dir: TempDir,
}

impl TempRepository {
    pub fn clone_recurse(url: &str, options: FetchOptions<'_>) -> Result<Self> {
        let tempdir = tempdir()?;
        trace!("Cloning {} into {}", url, tempdir.path().display());
        let repository = RepoBuilder::new().fetch_options(options).clone(url, tempdir.path())?;

        Ok(Self {
            repository,
            temp_dir: tempdir,
        })
    }
}

impl Deref for TempRepository {
    type Target = Repository;
    fn deref(&self) -> &Self::Target {
        &self.repository
    }
}

fn clone_composite(url: &str, branch: &Reference, options: FetchOptions<'_>) -> Result<TempRepository> {
    let repo = TempRepository::clone_recurse(url, options).unwrap();

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
        if let Ok(new_branch) = repo.branch(shorthand, &branch_commit, false) {
            debug!("Created branch {:?}", new_branch.name());
        } else {
            debug!("Reusing branch {:?}", shorthand);
        }
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
        .map(|mut x| {
            x.update(true, None).unwrap();
            (x.open().unwrap(), x)

            // run --package deployment-withlazers --bin deploy -- pipeline --composite-repository=/tmp/test-repos/composite-bare --repository=/tmp/test-repos/composite/micro-service-gary
        })
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


fn push_composite(composite_repo: &TempRepository, mut options: PushOptions) -> Result<()> {
    let mut remote = composite_repo.find_remote("origin")?;
    let callbacks = git2::RemoteCallbacks::new();
    // TODO: Add authentication and error reporting
    options.remote_callbacks(callbacks);

    // https://docs.rs/git2/latest/git2/struct.RemoteCallbacks.html
    // git -c http.https://<url of submodule repository>.extraheader="AUTHORIZATION: basic <BASE64_ENCODED_TOKEN_DESCRIBED_ABOVE>" submodule update --init --recursive
    // https://learn.microsoft.com/en-us/azure/devops/pipelines/repos/pipeline-options-for-git?view=azure-devops&tabs=yaml#alternative-to-using-the-checkout-submodules-option
    let head = composite_repo.head()?;
    if !head.is_branch() {
        return Err("Composite repository is not on a branch".into());
    }

    remote.push(&[head.name().unwrap()], Some(&mut options))?;
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

fn resolve_git_fetch<'a>(args: &Args) -> Result<git2::FetchOptions<'a>> {
    let mut options = git2::FetchOptions::new();
    options.custom_headers(&to_collect(args));
    Ok(options)
}

fn resolve_git_push<'a>(args: &Args) -> Result<git2::PushOptions<'a>> {
    let mut options = git2::PushOptions::new();
    options.custom_headers(&to_collect(&args));
    Ok(options)
}

fn to_collect(args: &Args) -> Vec<&str> {
    args.custom_headers.iter().map(|x| x.as_str()).collect::<Vec<&str>>()
}

pub fn run(args: Args) -> Result<()> {
    let repo = Repository::open(args.repository.clone())?;

    let head = repo.head()?;
    if !head.is_branch() {
        return Err("HEAD is not a branch".into());
    }

    let push_options = resolve_git_push(&args)?;
    let fetch_options = resolve_git_fetch(&args)?;

    let composite_repo = clone_composite(&args.composite_repository, &head, fetch_options)?;

    let mut submodule = find_submodule(&composite_repo, &head)?;

    let commit = head.peel_to_commit()?;

    update_submodule(&mut submodule, commit.id())?;

    commit_composite(&composite_repo, &submodule, &commit)?;

    // push_composite(&composite_repo, resolve_git_auth(args))?;
    push_composite(&composite_repo, push_options)?;

    Ok(())
}

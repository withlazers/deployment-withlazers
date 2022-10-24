use crate::result::Result;
use clap::Parser;
use git2::build::RepoBuilder;
use git2::{BranchType, FetchOptions, Oid, Repository, Submodule};
use log::{info, trace};
use tempfile::{tempdir, TempDir};

#[derive(Parser, Debug, Clone)]
pub struct Args {
    /// Repository that is updated
    #[arg(short, long, default_value = ".")]
    repository: String,

    /// Branch to updated
    #[arg(short, long)]
    git_ref: Option<String>,

    /// The composite repository
    #[arg(short, long)]
    composite_repository: String,

    /// Set custom headers for pulling and pushing
    #[arg(short = 'C', long)]
    custom_headers: Vec<String>,
}

impl Args {
    fn custom_headers_ref(&self) -> Vec<&str> {
        self.custom_headers
            .iter()
            .map(|x| x.as_str())
            .collect::<Vec<&str>>()
    }
}

struct RepositoryWrapper<'a> {
    repository: Repository,
    args: &'a Args,
    #[allow(dead_code)]
    tempdir: Option<TempDir>,
}

impl<'a> RepositoryWrapper<'a> {
    pub fn clone(url: &str, args: &'a Args) -> Result<Self> {
        let mut fetch_options = FetchOptions::new();
        fetch_options.custom_headers(&args.custom_headers_ref());

        let tempdir = tempdir()?;
        trace!("Cloning {} into {}", url, tempdir.path().display());
        let repository = RepoBuilder::new()
            .fetch_options(fetch_options)
            .clone(url, tempdir.path())?;

        Ok(Self {
            repository,
            args,
            tempdir: Some(tempdir),
        })
    }

    fn git_ref(&self) -> Result<String> {
        let head = self.repository.head()?;
        if let Some(git_ref) = &self.args.git_ref {
            Ok(git_ref.to_string())
        } else if head.is_branch() {
            Ok(self.repository.head()?.name().unwrap().to_string())
        } else {
            Err("No branch name given and HEAD is not a branch".into())
        }
    }

    fn head_id(&self) -> Result<Oid> {
        let git_ref = self.git_ref()?;
        let reference = self.repository.find_reference(&git_ref)?;
        let commit = reference.peel_to_commit()?;
        if self.repository.head()?.peel_to_commit()?.id() == commit.id() {
            Ok(commit.id())
        } else {
            Err("HEAD is not on the given branch".into())
        }
    }

    pub fn open(path: &str, args: &'a Args) -> Result<Self> {
        let repository = Repository::open(path)?;
        Ok(Self {
            repository,
            args,
            tempdir: None,
        })
    }

    /// branch_name is the full branch name containing the remote name (i.e. `refs/heads/main`)
    pub fn checkout_temp_branch(&self, git_ref: &str) -> Result<()> {
        trace!(
            "Checking out {} in {}",
            git_ref,
            self.repository.path().display()
        );
        let branch_name = Self::get_branch_name_from_ref(git_ref)?;
        let branch = self.repository.find_branch(
            &format!("origin/{}", branch_name),
            BranchType::Remote,
        );
        let commit = match branch {
            Ok(branch) => {
                trace!("Found reference {}.", git_ref);
                branch.get().peel_to_commit()?
            }
            Err(e) => {
                trace!("Did not find reference {}, using head: {}", git_ref, e);
                self.repository.head()?.peel_to_commit()?
            }
        };

        trace!(
            "Creating __temporary__ branch in {:?}",
            self.repository.path()
        );
        self.repository.branch("__temporary__", &commit, true)?;
        self.repository.set_head("refs/heads/__temporary__")?;
        self.repository.checkout_head(None)?;
        Ok(())
    }

    fn find_submodule_by_id(&self, id: Oid) -> Result<Submodule<'_>> {
        let submodules = self.repository.submodules()?;
        let (_repository, submodule) = submodules
            .into_iter()
            .map(|mut x| {
                x.update(true, None).unwrap();
                (x.open().unwrap(), x)
            })
            .inspect(|(_, x)| trace!("Found submodule {}", x.name().unwrap(),))
            .find(|(repository, _)| repository.find_commit(id).is_ok())
            .ok_or("No submodule found")?;
        info!("Found submodule: {:?}", submodule.path());
        Ok(submodule)
    }

    fn update_submodule_to_id(
        &self,
        submodule: &mut Submodule,
        id: Oid,
    ) -> Result<()> {
        let sub_repository = submodule.open()?;
        let commit = sub_repository.find_commit(id)?;
        info!("Found commit: {:?}", commit);
        sub_repository.set_head_detached(commit.id())?;
        submodule.add_to_index(true)?;
        info!("Updated {:?}", sub_repository.path());
        self.commit(submodule)?;
        Ok(())
    }

    fn commit(&self, submodule: &Submodule) -> Result<()> {
        let mut index = self.repository.index()?;
        let tree_id = index.write_tree()?;
        let tree = self.repository.find_tree(tree_id)?;
        let head = self.repository.head()?.peel_to_commit()?;
        let commit = self.repository.find_commit(head.id())?;
        let submodule_repo = submodule.open()?;
        let submodule_commit = submodule_repo.head()?.peel_to_commit()?;
        let message = format!(
            "Update submodule {} to {}\n---\n{}",
            submodule.path().display(),
            submodule_commit.id(),
            submodule_commit.message().unwrap()
        );
        self.repository.commit(
            Some("HEAD"),
            &submodule_commit.author(),
            &submodule_commit.committer(),
            &message,
            &tree,
            &[&commit],
        )?;
        Ok(())
    }

    fn get_branch_name_from_ref(git_ref: &str) -> Result<&str> {
        let prefix = "refs/heads/";
        if let Some(branch_name) = git_ref.strip_prefix(prefix) {
            Ok(branch_name)
        } else {
            Err("Invalid git ref".into())
        }
    }

    fn push(&self, git_ref_target: &str) -> Result<()> {
        let branch_name = Self::get_branch_name_from_ref(git_ref_target)?;
        let mut remote = self.repository.find_remote("origin")?;
        let mut options = git2::PushOptions::new();
        options.custom_headers(&self.args.custom_headers_ref());

        // https://docs.rs/git2/latest/git2/struct.RemoteCallbacks.html
        // git -c http.https://<url of submodule repository>.extraheader="AUTHORIZATION: basic <BASE64_ENCODED_TOKEN_DESCRIBED_ABOVE>" submodule update --init --recursive
        // https://learn.microsoft.com/en-us/azure/devops/pipelines/repos/pipeline-options-for-git?view=azure-devops&tabs=yaml#alternative-to-using-the-checkout-submodules-option
        let head = self.repository.head()?;
        if !head.is_branch() {
            return Err("Composite repository is not on a branch".into());
        }

        println!("Pushing to {}", branch_name);
        remote.push(
            &[format!(
                "refs/heads/__temporary__:refs/heads/{}",
                branch_name
            )],
            Some(&mut options),
        )?;
        Ok(())
    }
}

pub fn run(args: Args) -> Result<()> {
    let child_repository = RepositoryWrapper::open(&args.repository, &args)?;

    let git_ref = child_repository.git_ref()?;
    let child_head_oid = child_repository.head_id()?;

    let composite_repo =
        RepositoryWrapper::clone(&args.composite_repository, &args)?;

    composite_repo.checkout_temp_branch(&git_ref)?;

    let mut submodule = composite_repo.find_submodule_by_id(child_head_oid)?;

    composite_repo.update_submodule_to_id(&mut submodule, child_head_oid)?;

    composite_repo.push(&git_ref)?;

    Ok(())
}

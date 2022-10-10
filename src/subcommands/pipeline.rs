use crate::result::Result;
use clap::Parser;
use git2::{ObjectType, Repository};

#[derive(Parser, Debug, Clone)]
pub struct Args {
    #[arg(short, long, default_value = ".")]
    repository: String,
    #[arg(short, long)]
    composite_repository: String,
}

pub fn run(args: Args) -> Result<()> {
    let repo = Repository::open(args.repository)?;

    let head = repo.head()?;
    if !head.is_branch() {
        return Err("Not a branch".into());
    }
    let name = head.shorthand().unwrap();

    let composite_repo =
        Repository::clone_recurse(&args.composite_repository, "__composite")
            .unwrap();
    let composite_branch =
        match composite_repo.find_branch(name, git2::BranchType::Local) {
            Ok(branch) => branch,
            Err(_) => composite_repo.branch(
                name,
                &composite_repo
                    .head()?
                    .peel(ObjectType::Commit)?
                    .into_commit()
                    .unwrap(),
                false,
            )?,
        };
    let composite_object = composite_branch
        .into_reference()
        .peel(ObjectType::Commit)
        .unwrap();
    composite_repo.checkout_tree(&composite_object, None)?;

    let binding = composite_repo.submodules()?;
    let (submodule, repository) = binding
        .iter()
        .map(|x| (x, x.open().unwrap()))
        .find(|(_, repository)| {
            repository.find_commit(head.target().unwrap()).is_ok()
        })
        .ok_or("No submodule found")?;

    let commit = repository.find_commit(head.peel(ObjectType::Commit)?.id())?;
    repository.checkout_tree(commit.as_object(), None)?;
    println!("Checked out {:?}", repository.path());
    Ok(())
}

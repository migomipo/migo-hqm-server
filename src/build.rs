use vergen_git2::{Emitter, Git2Builder};

pub fn main() -> anyhow::Result<()> {
    let git2 = Git2Builder::default().sha(false).build()?;

    Emitter::default().add_instructions(&git2)?.emit()?;
    Ok(())
}

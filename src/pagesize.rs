use core::convert::TryFrom;
use nix::unistd::{sysconf, SysconfVar::PAGE_SIZE};
use once_cell::sync::OnceCell;
use snafu::{OptionExt, ResultExt, Snafu};

#[derive(Snafu, Debug)]
pub enum Error {
    #[snafu(display("Unable to get page size: {}", source))]
    PageSize { source: nix::Error },

    #[snafu(display("System reported incorrect page size: {:?}", size))]
    IncorrectPageSize { size: Option<i64> },
}

/// Gets and caches pagesize.
pub fn get_pagesize() -> Result<usize, Error> {
    static SIZE: OnceCell<usize> = OnceCell::new();

    if let Some(&pagesize) = SIZE.get() {
        Ok(pagesize)
    } else {
        let maybe_pagesize = sysconf(PAGE_SIZE).context(PageSize)?;
        let pagesize = maybe_pagesize
            .and_then(|size_i64| usize::try_from(size_i64).ok())
            .context(IncorrectPageSize {
                size: maybe_pagesize,
            })?;
        Ok(*SIZE.get_or_init(|| pagesize))
    }
}

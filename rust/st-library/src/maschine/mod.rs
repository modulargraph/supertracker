//! Maschine group import (written last).

use crate::api::KitReadResponse;
use crate::error::{Error, Result};

pub(crate) fn read_group(_path: &str, _root: Option<&str>) -> Result<KitReadResponse> {
    Err(Error::invalid("Maschine groups cannot be imported yet."))
}

//! Linux/glibc resolver ABI is selected at runtime: old webOS exposes
//! __res_query in libresolv; newer webOS exposes res_query from libc.
//! No new DT_NEEDED entry or assumed resolver-state struct layout.
use super::{Error, wire};
use std::io::Read;
const MAX_CONFIG_BYTES: u64 = 65536;

pub(super) fn domains() -> Vec<String> {
    for path in ["/etc/resolv.conf", "/var/run/connman/resolv.conf"] {
        if let Ok(file) = std::fs::File::open(path) {
            let mut bytes = Vec::new();
            if file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes).is_ok()
                && bytes.len() <= MAX_CONFIG_BYTES as usize
            {
                if let Ok(text) = std::str::from_utf8(&bytes) { return parse_domains(text); }
            }
        }
    }
    Vec::new()
}
fn parse_domains(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    for line in text.lines() {
        let mut parts = line.split(['#',';']).next().unwrap_or("").split_whitespace();
        match parts.next() {
            Some("search") => result = parts.map(str::to_owned).collect(),
            Some("domain") => result = parts.next().map(str::to_owned).into_iter().collect(),
            _ => {}
        }
    }
    result
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{ffi::CString, os::raw::{c_char,c_int,c_uchar,c_void}, sync::OnceLock};
    use plx_base::dynlib::Handle;
    type Query = unsafe extern "C" fn(*const c_char,c_int,c_int,*mut c_uchar,c_int)->c_int;
    #[derive(Clone, Copy)]
    struct Api { query: Query, search: Query }
    static API: OnceLock<Option<Api>> = OnceLock::new();
    fn api() -> Option<Api> {
        *API.get_or_init(|| {
            let names = ["libresolv.so.2", "libc.so.6"];
            let symbol = |candidates: &[&str]| {
                names.iter().filter_map(|name| Handle::open(&[*name]).map(|v|v.0))
                    .chain(std::iter::once(Handle::self_handle()))
                    .find_map(|handle| candidates.iter().find_map(|s|handle.sym(s).filter(|p| !p.is_null())))
            };
            let query = symbol(&["__res_query","res_query"])?;
            let search = symbol(&["__res_search","res_search"])?;
            // SAFETY: both symbol families have the five-argument resolver ABI
            // verified in the pinned ARM SDK and all 14 firmware inventories.
            // Handle never dlcloses, so these pointers remain valid process-wide.
            Some(Api { query: unsafe { std::mem::transmute::<*mut c_void, Query>(query) },
                search: unsafe { std::mem::transmute::<*mut c_void, Query>(search) } })
        })
    }
    pub(super) fn query(name: &str, kind: u16, search: bool) -> Result<wire::Answer, Error> {
        let api = api().ok_or(Error::Unsupported)?;
        let cname = CString::new(name).map_err(|_|Error::Malformed)?;
        // DNS TCP message size. One allocation and no retry on truncation.
        let mut bytes = vec![0;65535];
        // SAFETY: CString and initialized output live through this blocking call;
        // the exact allocation length fits c_int on ARM32 and the host.
        let length = unsafe { (if search { api.search } else { api.query })(
            cname.as_ptr(),1,c_int::from(kind),bytes.as_mut_ptr(),65535) };
        if length < 0 { return wire::negative(&bytes[..12],name); }
        let length=usize::try_from(length).map_err(|_|Error::Malformed)?;
        if length >= bytes.len() { return Err(Error::TooLarge); }
        wire::parse(&bytes[..length],name,kind,search)
    }
}
pub(super) fn query(name: &str, kind: u16, search: bool) -> Result<wire::Answer, Error> {
    #[cfg(target_os = "linux")]
    { linux::query(name,kind,search) }
    #[cfg(not(target_os = "linux"))]
    { let _=(name,kind,search); Err(Error::Unsupported) }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolver_search_precedence_and_comments() {
        assert_eq!(parse_domains("search first second # ignore\ndomain final;ignore"),vec!["final"]);
        assert_eq!(parse_domains("domain first\nsearch lan.example local"),vec!["lan.example","local"]);
    }
}

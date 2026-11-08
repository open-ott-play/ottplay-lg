use super::{identity, text, Catalog, CatalogEntry, CatalogError, MediaLocator, ProviderKind, ProviderProfile, MAX_CATALOG_ENTRIES};
use serde::{Deserialize,Serialize};

/// The IO runner must check canonical containment and exclude symlinks before creating a row.
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalFile { pub relative_path:String, pub size:u64, #[serde(default)] pub duration_ms:Option<u64> }

/// Display text from a receiver-owned source. It is never a path, opaque token
/// or legacy name selector. Preserve its bytes instead of trimming or decoding.
pub fn valid_local_source_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

/// Relative media identity, independent of the receiving device's selected root.
pub fn valid_local_relative_path(path: &str) -> bool {
    !path.is_empty() && path.len() <= 8192 && !path.starts_with('/')
        && !path.contains(['\\', '\0']) && !path.chars().any(char::is_control)
        && !path.split('/').any(|s| s.is_empty() || s == "." || s == "..")
        && !(path.len() > 1 && path.as_bytes()[1] == b':')
}

/// Admission to the catalogue is not a promise that the device decodes this container.
/// The media adapter probes the selected original before starting its native decoder.
pub fn is_local_video_path(path: &str) -> bool {
    let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(extension.as_str(), "mp4" | "m4v" | "mkv" | "avi" | "mov" | "webm"
        | "ts" | "m2ts" | "mts" | "mpg" | "mpeg" | "vob" | "wmv" | "flv" | "ogv")
}

pub fn catalog_from_local_files(files:&[LocalFile],profile:&ProviderProfile)->Result<Catalog,CatalogError> {
    profile.validate()?;
    if profile.kind!=ProviderKind::Local {return Err(CatalogError::InvalidProfile);}
    if files.len()>MAX_CATALOG_ENTRIES{return Err(CatalogError::LimitExceeded);}
    let mut entries=Vec::new();
    for file in files {
        let path=&file.relative_path;
        if !valid_local_relative_path(path) {return Err(CatalogError::InvalidPath);}
        if !is_local_video_path(path) {continue;}
        let (folder,name)=path.rsplit_once('/').unwrap_or(("",path));
        entries.push(CatalogEntry{synopsis:None,id:identity(&profile.id,path),title:text(name,1024),category:if folder.is_empty(){profile.name.clone()}else{text(folder,4096)},description:String::new(),poster:String::new(),media:MediaLocator::Local{relative_path:path.clone()},duration_ms:file.duration_ms,provider_id_field: None, live: None, provider_id:None});
    }
    let mut catalog=Catalog::default();catalog.merge(Catalog{ hierarchy: None,entries})?;Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile()->ProviderProfile {ProviderProfile{id:"local-home".into(),name:"Videos".into(),kind:ProviderKind::Local,source:"/media/usb/videos".into(),source_alias:"Home".into(),enabled:true}}
    #[test]fn nested_sources_retain_relative_path_and_formats(){
        let files=vec![LocalFile{relative_path:"Family/One.MKV".into(),size:100,duration_ms:Some(5000)},LocalFile{relative_path:"Two.avi".into(),size:100,duration_ms:None},LocalFile{relative_path:"ignored.txt".into(),size:1,duration_ms:None}];
        let catalog=catalog_from_local_files(&files,&profile()).unwrap();assert_eq!(catalog.entries.len(),2);assert_eq!(catalog.categories(),vec!["Family","Videos"]);assert_eq!(catalog.entries[0].media,MediaLocator::Local{relative_path:"Family/One.MKV".into()});
    }
    #[test]fn path_escape_is_rejected(){for path in ["../private.mp4","/private.mp4","sub/../../private.mp4","C:/private.mp4","sub\\private.mp4","sub//file.mp4","./file.mp4","sub/\nfile.mp4"]{assert_eq!(catalog_from_local_files(&[LocalFile{relative_path:path.into(),size:1,duration_ms:None}],&profile()),Err(CatalogError::InvalidPath));}}
    #[test]fn identity_survives_batch_order_and_root_relocation(){
        let one=LocalFile{relative_path:"Folder/one.mp4".into(),size:42,duration_ms:None};
        let two=LocalFile{relative_path:"Folder/two.mp4".into(),size:99,duration_ms:None};
        let first=catalog_from_local_files(&[one.clone(),two.clone()],&profile()).unwrap();
        let mut relocated=profile();relocated.source="/media/other-device/videos".into();
        let reversed=catalog_from_local_files(&[two,one],&relocated).unwrap();
        assert_eq!(first.entries[0].id,reversed.entries[1].id);
        assert_ne!(first.entries[0].id,first.entries[1].id);
    }
}


#[cfg(test)]
mod source_label_tests {
    use super::*;
    #[test]
    fn labels_are_not_path_or_selector_authority() {
        for label in [".",".."," ","back\\slash","親 / Movies"] {
            assert!(valid_local_source_label(label));
            let profile=ProviderProfile{id:"local-label".into(),name:"Local videos".into(),kind:ProviderKind::Local,source:"/media/usb/Movies".into(),source_alias:label.into(),enabled:true};
            assert!(profile.validate().is_ok());
            assert_eq!(profile.source_alias,label);
            assert_eq!(catalog_from_local_files(&[LocalFile{relative_path:"one.mp4".into(),size:1,duration_ms:None}],&profile).unwrap().entries.len(),1);
        }
        assert!(valid_local_source_label(&"я".repeat(256)));
        for value in [String::new(),"line\nnext".into(),"x\u{85}y".into(),"я".repeat(257)] {assert!(!valid_local_source_label(&value));}
        for path in [".","..","back\\slash","folder/../file.mp4"] {assert!(!valid_local_relative_path(path));}
    }
}

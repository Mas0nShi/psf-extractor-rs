use std::{fs, io::{Read, Seek, Write}, os::windows::io::AsRawHandle, path::{Path, PathBuf}};
use quick_xml;
use serde::Deserialize;
use windows::Win32::{Foundation::{FALSE, FILETIME, HANDLE}, Storage::FileSystem::SetFileTime};
use windows::Win32::System::ApplicationInstallationAndServicing::{ApplyDeltaB, DeltaFree, DELTA_INPUT, DELTA_INPUT_0, DELTA_OUTPUT};

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("psf_extractor/include/extractor.h");
        fn extract(file_name: &str, file_dir: &str, out_path: &str) -> bool;
    }
}

type IResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, PartialEq, Deserialize)]
struct Description;

#[derive(Debug, PartialEq, Deserialize)]
struct Location {
    #[serde(rename = "@id")]
    id: i64,
    #[serde(rename = "@path")]
    path: String,
    #[serde(rename = "@flags")]
    flags: u64,
}


#[derive(Debug, PartialEq, Deserialize)]
struct DeltaBasisSearch {
    #[serde(rename = "Location")]
    location: Vec<Location>,
}

#[derive(Debug, PartialEq, Deserialize)]
struct Hash {
    #[serde(rename = "@alg")]
    alg: String,
    #[serde(rename = "@value")]
    value: String,
}

// type: PA30 or PA18 or RAW
#[derive(Debug, PartialEq, Deserialize)]
enum SourceType {
    PA30,
    PA19,
    RAW,
}

#[derive(Debug, PartialEq, Deserialize)]
struct Source {
    #[serde(rename = "@type")]
    type_: SourceType,
    #[serde(rename = "@offset")]
    offset: u64,
    #[serde(rename = "@length")]
    length: usize,
    #[serde(rename = "Hash")]
    hash: Hash,
}

#[derive(Debug, PartialEq, Deserialize)]
struct Delta {
    #[serde(rename = "Source")]
    source: Source,
}


#[derive(Debug, PartialEq, Deserialize)]
struct File {
    #[serde(rename = "@id")]
    id: u64,
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@length")]
    length: usize,
    #[serde(rename = "@time")]
    time: u64,
    #[serde(rename = "@attr")]
    attr: u64,
    #[serde(rename = "Hash")]
    hash: Hash,
    #[serde(rename = "Delta")]
    delta: Delta,
}

#[derive(Debug, PartialEq, Deserialize)]
struct Files {
    #[serde(rename = "File")]
    file: Vec<File>,
}
#[derive(Debug, PartialEq, Deserialize)]
struct Container {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@type")]
    type_: String,
    #[serde(rename = "@length")]
    length: usize,
    #[serde(rename = "@version")]
    version: String,
    #[serde(rename = "@xmlns")]
    xmlns: String,
    
    #[serde(rename = "Description")]
    description: Description,
    #[serde(rename = "DeltaBasisSearch")]
    delta_basis_search: DeltaBasisSearch,
    #[serde(rename = "Files")]
    files: Files,
}

fn find_desc_xml<P>(path: P) -> std::io::Result<PathBuf> where P: AsRef<Path> {
    let path = path.as_ref();

    let may_path = path.join("express.psf.cix.xml");
    if may_path.exists() {
        return Ok(may_path);
    }

    let mut desc_file = PathBuf::new();
    for entry in fs::read_dir(path)? {
        let file_name = entry?.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.ends_with(".psf.cix.xml") {
            desc_file = path.join(file_name.as_ref());
            break;
        }
    }

    Ok(desc_file)
}

fn parse_desc_xml<P>(path: P) -> IResult<Container> where P:AsRef<Path> {
    let file = fs::File::open(path.as_ref())?;
    let reader = std::io::BufReader::new(file);
    let container: Container = quick_xml::de::from_reader(reader)?;

    Ok(container)
}

fn expand_delta<P>(psf_file: P, container: &Container, output: P) -> IResult<()> where P: AsRef<Path>
{
    let psf_file = psf_file.as_ref()
    .canonicalize()?;
    let output = output.as_ref()
    .canonicalize()?;

    // buffer reader
    let file = fs::File::open(psf_file)?;
    let mut reader = std::io::BufReader::new(file);

    for file in &container.files.file {
        let name = file.name.as_str();
        let patch_type = &file.delta.source.type_;
        let offset = file.delta.source.offset;
        let length = file.delta.source.length;

        let out_file = output.join(name);
        let out_file_dir = out_file.parent()
        .ok_or("file parent not found")?;
        fs::create_dir_all(out_file_dir)?;

        reader.seek(std::io::SeekFrom::Start(offset))?;
        
        let mut buffer = vec![0; length];
        reader.read_exact(&mut buffer)?;

        let mut out_file = fs::File::create(out_file)?;

        match patch_type {
            SourceType::PA30 => {
                let delta_input = DELTA_INPUT {
                    Anonymous: DELTA_INPUT_0 {
                        lpcStart: buffer.as_ptr() as *const _,
                    },
                    uSize: length,
                    Editable: FALSE,
                };
                
                let delta_null_input = DELTA_INPUT::default();
                let mut delta_output = DELTA_OUTPUT::default();

                unsafe { 
                    ApplyDeltaB(0, delta_null_input, delta_input, &mut delta_output)
                }.expect("apply delta failed");

                let buffer = unsafe { std::slice::from_raw_parts(delta_output.lpStart as *const u8, delta_output.uSize) };
                out_file.write_all(&buffer)?;
                
                unsafe { 
                    DeltaFree(delta_output.lpStart)
                }.expect("free delta failed");
            }
            SourceType::PA19 => {
                unimplemented!("PA19 not implemented");
            }

            SourceType::RAW => {
                out_file.write_all(&buffer)?;
            }
        }
        
        let mtimes = FILETIME {
            dwLowDateTime: file.time as u32,
            dwHighDateTime: (file.time >> 32) as u32,
        };
        
        unsafe { SetFileTime(HANDLE(out_file.as_raw_handle() as _),
            None, 
            None, 
            Some(&mtimes)) }
            .expect("set file time failed");
    }

    Ok(())
}


fn extract_cxx<P>(path: P, output: P) -> IResult<()> where P:AsRef<Path> {
    let path = path.as_ref().canonicalize()?;
    let _ = fs::create_dir_all(&output)?;
    let output = output.as_ref().canonicalize()?;

    let file_name = path.file_name()
    .ok_or("file name not found")?
    .to_string_lossy();
    let file_dir = path.parent()
    .ok_or("error parent not found")?
    .to_string_lossy();
    
    let output = output.to_string_lossy();

    let r = ffi::extract(&file_name, &file_dir, &output);
    assert!(r, "extract failed");

    Ok(())
}


pub fn extract_msu<P>(msu_file: P, output: P) -> IResult<()> where P: AsRef<Path> {
    extract_cxx(msu_file.as_ref(), output.as_ref())
}

pub fn extract_cab<P>(cab_file: P, output: P) -> IResult<()> where P: AsRef<Path> {
    extract_cxx(cab_file.as_ref(), output.as_ref())
}

pub fn extract_cab_with_psf<P>(cab_file: P, psf_file: P, output: P) -> IResult<()> where P: AsRef<Path> {
    extract_cab(cab_file.as_ref(), output.as_ref())?;
    // find desc file
    let desc_file = find_desc_xml(output.as_ref())?;
    let container = parse_desc_xml(desc_file.as_path())?;

    expand_delta(psf_file.as_ref(), &container, output.as_ref())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_extractor_msu() {
        let path = r"tests\test.msu";
        let output = r"tests\msu_extracted";
        extract_msu(path, output)
        .expect("extract msu failed");
    }

    #[test]
    fn test_extractor_cab() {
        let path = r"tests\test.cab";
        let output = r"tests\cab_extracted_only";
        extract_cab(path, output)
        .expect("extract cab failed");
    }

    #[test]
    fn test_extractor_cab_with_psf() {
        let cab_path = r"tests\test.cab";
        let psf_path = r"tests\test.psf";
        let output = r"tests\cab_extracted_with_psf";
        extract_cab_with_psf(cab_path, psf_path, output)
        .expect("extract cab with psf failed");
    }
}
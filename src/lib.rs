use std::{borrow::Borrow, collections::HashSet, fs, io::{ErrorKind, Read, Seek, Write}, os::windows::io::AsRawHandle, path::{Path, PathBuf}};
use cab::{Cabinet, FileReader};
use quick_xml;
use serde::Deserialize;
use windows::Win32::{Foundation::{FALSE, FILETIME, HANDLE}, Storage::FileSystem::SetFileTime};
use windows::Win32::System::ApplicationInstallationAndServicing::{ApplyDeltaB, DeltaFree, DELTA_INPUT, DELTA_INPUT_0, DELTA_OUTPUT};

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

use std::rc::Rc;
use std::cell::RefCell;

struct Extractor<'a> {
    cab_file_reader: Rc<RefCell<Cabinet<FileReader<'a, fs::File>>>>,
    psf_file_reader: Rc<RefCell<FileReader<'a, FileReader<'a, fs::File>>>>,
}

impl<'a> Extractor<'a> {
    pub fn new<P>(src: P) -> IResult<Self>
 where P:AsRef<Path> {
        let src_file = fs::File::open(src.as_ref())?;
        let mut src_r = Cabinet::new(src_file)?;
    
        // re to match target cab.
        let re = regex::Regex::new(r"Windows(\d+\.\d+)-(KB\d+)-(.*)\.cab")?;
        let cab_name_with_ext = src_r.folder_entries()
        .flat_map(|folder| folder.file_entries())
        .find(|file| re.is_match(file.name()))
        .expect("cab not found")
        .name()
        .to_owned();

        let cab_name = Path::new(&cab_name_with_ext).file_stem()
        .unwrap()
        .to_string_lossy();
        
        println!("cab_name: {}", cab_name);
        let cab_name_psf = format!("{}.psf", cab_name);
        
        let cab_file = src_r.read_file(&cab_name_with_ext)?;
        let mut cab_r= Cabinet::new(cab_file)?;

        // for folder in cab_r.folder_entries() {
        //     for file in folder.file_entries() {
        //         println!("cab: {}", file.name());
        //     }
        // }

        let xml = cab_r.read_file("express.psf.cix.xml")?;
        let reader = std::io::BufReader::new(xml);
        let container: Container = quick_xml::de::from_reader(reader)?;
        // println!("{:?}", container);

        let psf_file = cab_r.read_file(&cab_name_psf)?;

        Ok(Extractor {
            cab_file_reader: Rc::new(RefCell::new(cab_r)),
            psf_file_reader: Rc::new(RefCell::new(psf_file)),

        })
    }

    pub fn new_s_pack<P>(src: P) -> IResult<Self> where P: AsRef<Path> {
        let src_file = fs::File::open(src.as_ref())?;
        let mut src_r = Cabinet::new(src_file)?;
    
        // re to match target cab.
        let re = regex::Regex::new(r"Windows(\d+\.\d+)-(KB\d+)-(.*)\.cab")?;
        let cab_name_with_ext = src_r.folder_entries()
        .flat_map(|folder| folder.file_entries())
        .find(|file| re.is_match(file.name()))
        .expect("cab not found")
        .name()
        .to_owned();

        let cab_name = Path::new(&cab_name_with_ext).file_stem()
        .unwrap()
        .to_string_lossy();
        
        println!("cab_name: {}", cab_name);
        
        let cab_file = src_r.read_file(&cab_name_with_ext)?;
        let mut cab_r= Cabinet::new(cab_file)?;
        // depth 1
        let cab_file = cab_r.read_file(&cab_name_with_ext)?;
        let mut cab_r = Cabinet::new(cab_file)?;



        for folder in cab_r.folder_entries() {
            for file in folder.file_entries() {
                println!("cab: {}", file.name());
            }
        }

        Ok(Extractor{
            
        })

    }
}


#[cfg(test)]
mod tests {
    use super::*;

    
    #[test]
    fn test_extract() {
        // win server 2022
        let src_path = r"C:\Users\mason\Downloads\windows10.0-kb5022842-x64_708d02971c761091c9d978a18588a315c3817343.msu";
        // let src_path = r"C:\Users\mason\Downloads\windows11.0-kb5019980-x64_8c5c341ffaa52f1e832bbd2a9acc5072c52b89fe.msu";
        let r = Extractor::new_s_pack(src_path).unwrap();

    }
}
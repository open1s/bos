use crate::{from_bytes, to_bytes, Deserialize, Error, Result, RkyvArchive, SerializeStrategy};
use std::io::{Read, Write};
use std::path::Path;

pub trait SerializeExt: RkyvArchive
where
    for<'a> Self: rkyv::Serialize<SerializeStrategy<'a>>,
{
    fn to_writer<W: Write>(&self, mut writer: W) -> Result<usize> {
        let bytes = to_bytes(self)?;
        writer
            .write_all(&bytes)
            .map_err(|e| Error::Serialize(rkyv::rancor::Error::new(e)))?;
        Ok(bytes.len())
    }

    fn to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let bytes = to_bytes(self)?;
        std::fs::write(path, &bytes).map_err(|e| Error::Serialize(rkyv::rancor::Error::new(e)))?;
        Ok(())
    }
}

impl<T> SerializeExt for T
where
    T: RkyvArchive,
    for<'a> T: rkyv::Serialize<SerializeStrategy<'a>>,
{
}

pub trait DeserializeExt2: Sized + Deserialize {
    fn from_reader<R: Read>(reader: R) -> Result<Self>;
    fn from_file<P: AsRef<Path>>(path: P) -> Result<Self>;
}

impl<T: Deserialize> DeserializeExt2 for T {
    fn from_reader<R: Read>(mut reader: R) -> Result<Self> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|e| Error::Deserialize(rkyv::rancor::Error::new(e)))?;
        from_bytes(&bytes)
    }

    fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let bytes =
            std::fs::read(path).map_err(|e| Error::Deserialize(rkyv::rancor::Error::new(e)))?;
        from_bytes(&bytes)
    }
}

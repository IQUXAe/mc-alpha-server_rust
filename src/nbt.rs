use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;

use crate::byte_buffer::{ByteBuffer, ByteBufferError};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

#[derive(Debug, Clone, PartialEq)]
pub enum NbtTag {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(NbtList),
    Compound(NbtCompound),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NbtTagType {
    End = 0,
    Byte = 1,
    Short = 2,
    Int = 3,
    Long = 4,
    Float = 5,
    Double = 6,
    ByteArray = 7,
    String = 8,
    List = 9,
    Compound = 10,
}

impl NbtTagType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::End),
            1 => Some(Self::Byte),
            2 => Some(Self::Short),
            3 => Some(Self::Int),
            4 => Some(Self::Long),
            5 => Some(Self::Float),
            6 => Some(Self::Double),
            7 => Some(Self::ByteArray),
            8 => Some(Self::String),
            9 => Some(Self::List),
            10 => Some(Self::Compound),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::End => "TAG_End",
            Self::Byte => "TAG_Byte",
            Self::Short => "TAG_Short",
            Self::Int => "TAG_Int",
            Self::Long => "TAG_Long",
            Self::Float => "TAG_Float",
            Self::Double => "TAG_Double",
            Self::ByteArray => "TAG_ByteArray",
            Self::String => "TAG_String",
            Self::List => "TAG_List",
            Self::Compound => "TAG_Compound",
        }
    }
}

impl TryFrom<u8> for NbtTagType {
    type Error = NbtError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match Self::from_u8(v) {
            Some(t) => Ok(t),
            None => Err(NbtError::UnknownTagType(v)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NbtError {
    Io(String),
    UnknownTagType(u8),
    NegativeByteArrayLength(i32),
    NegativeListLength(i32),
    NegativeStringLength(i16),
    StringTooLong(usize),
    ArrayTooLong(usize),
    InvalidRoot(String),
    InvalidUtf8(String),
    Buffer(String),
}

impl fmt::Display for NbtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "NBT IO error: {msg}"),
            Self::UnknownTagType(t) => write!(f, "NBT readTag: unknown tag type {t}"),
            Self::NegativeByteArrayLength(n) => {
                write!(f, "NBT readTag: negative byte array length {n}")
            }
            Self::NegativeListLength(n) => write!(f, "NBT readTag: negative list count {n}"),
            Self::NegativeStringLength(n) => {
                write!(f, "NBT readTag: negative string length {n}")
            }
            Self::StringTooLong(n) => write!(f, "NBT string too long: {n} bytes"),
            Self::ArrayTooLong(n) => write!(f, "NBT array too long: {n} bytes"),
            Self::InvalidRoot(msg) => write!(f, "NBT invalid root: {msg}"),
            Self::InvalidUtf8(msg) => write!(f, "NBT invalid UTF-8: {msg}"),
            Self::Buffer(msg) => write!(f, "NBT buffer error: {msg}"),
        }
    }
}

impl std::error::Error for NbtError {}

impl From<std::io::Error> for NbtError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<ByteBufferError> for NbtError {
    fn from(e: ByteBufferError) -> Self {
        match e {
            ByteBufferError::Underflow { needed, remaining } => Self::Buffer(format!(
                "Buffer underflow: needed {needed}, remaining {remaining}"
            )),
            ByteBufferError::StringTooLong(n) => Self::StringTooLong(n),
            ByteBufferError::NegativeLength(n) => {
                if n < 0 {
                    Self::NegativeStringLength(n as i16)
                } else {
                    Self::Buffer(format!("Negative length {n}"))
                }
            }
            ByteBufferError::LengthExceedsMaximum { len, max } => {
                Self::Buffer(format!("UTF length {len} exceeds maximum {max}"))
            }
            ByteBufferError::InvalidUtf8 => Self::InvalidUtf8("invalid UTF-8".to_string()),
        }
    }
}

impl From<NbtError> for std::io::Error {
    fn from(e: NbtError) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    }
}

impl NbtTag {
    pub fn tag_type(&self) -> u8 {
        match self {
            Self::End => 0,
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(_) => 9,
            Self::Compound(_) => 10,
        }
    }

    pub fn tag_type_enum(&self) -> NbtTagType {
        match self {
            Self::End => NbtTagType::End,
            Self::Byte(_) => NbtTagType::Byte,
            Self::Short(_) => NbtTagType::Short,
            Self::Int(_) => NbtTagType::Int,
            Self::Long(_) => NbtTagType::Long,
            Self::Float(_) => NbtTagType::Float,
            Self::Double(_) => NbtTagType::Double,
            Self::ByteArray(_) => NbtTagType::ByteArray,
            Self::String(_) => NbtTagType::String,
            Self::List(_) => NbtTagType::List,
            Self::Compound(_) => NbtTagType::Compound,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NbtCompound {
    pub map: BTreeMap<String, NbtTag>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NbtList {
    pub tag_type: u8,
    pub elements: Vec<NbtTag>,
}

impl NbtCompound {
    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<NbtTag> {
        self.map.remove(name)
    }

    pub fn set_tag(&mut self, name: &str, tag: Option<NbtTag>) {
        match tag {
            Some(t) => {
                self.map.insert(name.to_owned(), t);
            }
            None => {
                self.map.remove(name);
            }
        }
    }

    pub fn set_byte(&mut self, name: &str, v: i8) {
        self.map.insert(name.to_owned(), NbtTag::Byte(v));
    }

    pub fn set_short(&mut self, name: &str, v: i16) {
        self.map.insert(name.to_owned(), NbtTag::Short(v));
    }

    pub fn set_int(&mut self, name: &str, v: i32) {
        self.map.insert(name.to_owned(), NbtTag::Int(v));
    }

    pub fn set_long(&mut self, name: &str, v: i64) {
        self.map.insert(name.to_owned(), NbtTag::Long(v));
    }

    pub fn set_float(&mut self, name: &str, v: f32) {
        self.map.insert(name.to_owned(), NbtTag::Float(v));
    }

    pub fn set_double(&mut self, name: &str, v: f64) {
        self.map.insert(name.to_owned(), NbtTag::Double(v));
    }

    pub fn set_string(&mut self, name: &str, v: &str) {
        self.map
            .insert(name.to_owned(), NbtTag::String(v.to_owned()));
    }

    pub fn set_byte_array(&mut self, name: &str, v: Vec<u8>) {
        self.map.insert(name.to_owned(), NbtTag::ByteArray(v));
    }

    pub fn set_byte_array_slice(&mut self, name: &str, v: &[u8]) {
        self.map
            .insert(name.to_owned(), NbtTag::ByteArray(v.to_vec()));
    }

    pub fn set_compound(&mut self, name: &str, v: Option<NbtCompound>) {
        match v {
            Some(c) => {
                self.map.insert(name.to_owned(), NbtTag::Compound(c));
            }
            None => {
                self.map.remove(name);
            }
        }
    }

    pub fn set_list(&mut self, name: &str, v: Option<NbtList>) {
        match v {
            Some(l) => {
                self.map.insert(name.to_owned(), NbtTag::List(l));
            }
            None => {
                self.map.remove(name);
            }
        }
    }

    pub fn set_boolean(&mut self, name: &str, v: bool) {
        self.map
            .insert(name.to_owned(), NbtTag::Byte(if v { 1 } else { 0 }));
    }

    pub fn get_byte(&self, name: &str) -> i8 {
        match self.map.get(name) {
            Some(NbtTag::Byte(v)) => *v,
            _ => 0,
        }
    }

    pub fn get_short(&self, name: &str) -> i16 {
        match self.map.get(name) {
            Some(NbtTag::Short(v)) => *v,
            _ => 0,
        }
    }

    pub fn get_int(&self, name: &str) -> i32 {
        match self.map.get(name) {
            Some(NbtTag::Int(v)) => *v,
            _ => 0,
        }
    }

    pub fn get_long(&self, name: &str) -> i64 {
        match self.map.get(name) {
            Some(NbtTag::Long(v)) => *v,
            _ => 0,
        }
    }

    pub fn get_float(&self, name: &str) -> f32 {
        match self.map.get(name) {
            Some(NbtTag::Float(v)) => *v,
            _ => 0.0,
        }
    }

    pub fn get_double(&self, name: &str) -> f64 {
        match self.map.get(name) {
            Some(NbtTag::Double(v)) => *v,
            _ => 0.0,
        }
    }

    pub fn get_string(&self, name: &str) -> String {
        match self.map.get(name) {
            Some(NbtTag::String(s)) => s.clone(),
            _ => String::new(),
        }
    }

    pub fn get_byte_array(&self, name: &str) -> Vec<u8> {
        match self.map.get(name) {
            Some(NbtTag::ByteArray(v)) => v.clone(),
            _ => Vec::new(),
        }
    }

    pub fn get_compound(&self, name: &str) -> Option<&NbtCompound> {
        match self.map.get(name) {
            Some(NbtTag::Compound(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_compound_mut(&mut self, name: &str) -> Option<&mut NbtCompound> {
        match self.map.get_mut(name) {
            Some(NbtTag::Compound(c)) => Some(c),
            _ => None,
        }
    }

    pub fn get_list(&self, name: &str) -> Option<&NbtList> {
        match self.map.get(name) {
            Some(NbtTag::List(l)) => Some(l),
            _ => None,
        }
    }

    pub fn get_list_mut(&mut self, name: &str) -> Option<&mut NbtList> {
        match self.map.get_mut(name) {
            Some(NbtTag::List(l)) => Some(l),
            _ => None,
        }
    }

    pub fn get_boolean(&self, name: &str) -> bool {
        self.get_byte(name) != 0
    }
}

impl Default for NbtCompound {
    fn default() -> Self {
        Self::new()
    }
}

impl NbtList {
    pub fn new() -> Self {
        Self {
            tag_type: 0,
            elements: Vec::new(),
        }
    }

    pub fn with_type(tag_type: u8) -> Self {
        Self {
            tag_type,
            elements: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn push(&mut self, tag: NbtTag) {
        if self.elements.is_empty() {
            self.tag_type = tag.tag_type();
        }
        self.elements.push(tag);
    }

    pub fn get(&self, idx: usize) -> Option<&NbtTag> {
        self.elements.get(idx)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, NbtTag> {
        self.elements.iter()
    }
}

impl Default for NbtList {
    fn default() -> Self {
        Self::new()
    }
}

pub fn tag_type_name(tag_type: u8) -> &'static str {
    match NbtTagType::from_u8(tag_type) {
        Some(t) => t.name(),
        None => "TAG_Unknown",
    }
}

pub fn is_gzip_magic(data: &[u8]) -> bool {
    match (data.first(), data.get(1)) {
        (Some(&a), Some(&b)) => a == 0x1F && b == 0x8B,
        _ => false,
    }
}

pub fn read_string<R: Read>(reader: &mut R) -> std::io::Result<String> {
    let mut len_buf = [0u8; 2];
    reader.read_exact(&mut len_buf)?;
    let len_signed = i16::from_be_bytes(len_buf);
    if len_signed < 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("NBT readTag: negative string length {len_signed}"),
        ));
    }
    let len = len_signed as usize;
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn write_string<W: Write>(writer: &mut W, s: &str) -> std::io::Result<()> {
    let bytes = s.as_bytes();
    let len = bytes.len() as u16;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(bytes)
}

pub fn read_payload<R: Read>(reader: &mut R, tag_type: u8) -> std::io::Result<NbtTag> {
    match tag_type {
        0 => Ok(NbtTag::End),
        1 => {
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            Ok(NbtTag::Byte(buf[0] as i8))
        }
        2 => {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            Ok(NbtTag::Short(i16::from_be_bytes(buf)))
        }
        3 => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            Ok(NbtTag::Int(i32::from_be_bytes(buf)))
        }
        4 => {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            Ok(NbtTag::Long(i64::from_be_bytes(buf)))
        }
        5 => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            Ok(NbtTag::Float(f32::from_be_bytes(buf)))
        }
        6 => {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            Ok(NbtTag::Double(f64::from_be_bytes(buf)))
        }
        7 => {
            let mut len_buf = [0u8; 4];
            reader.read_exact(&mut len_buf)?;
            let len = i32::from_be_bytes(len_buf);
            if len < 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("NBT readTag: negative byte array length {len}")));
            }
            let mut bytes = vec![0u8; len as usize];
            reader.read_exact(&mut bytes)?;
            Ok(NbtTag::ByteArray(bytes))
        }
        8 => {
            let s = read_string(reader)?;
            Ok(NbtTag::String(s))
        }
        9 => {
            let mut type_buf = [0u8; 1];
            reader.read_exact(&mut type_buf)?;
            let tag_type = type_buf[0];
            let mut len_buf = [0u8; 4];
            reader.read_exact(&mut len_buf)?;
            let len = i32::from_be_bytes(len_buf);
            if len < 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("NBT readTag: negative list count {len}")));
            }
            let mut elements = Vec::with_capacity(len as usize);
            for _ in 0..len {
                elements.push(read_payload(reader, tag_type)?);
            }
            Ok(NbtTag::List(NbtList { tag_type, elements }))
        }
        10 => {
            let mut map = BTreeMap::new();
            loop {
                let mut type_buf = [0u8; 1];
                reader.read_exact(&mut type_buf)?;
                let tag_type = type_buf[0];
                if tag_type == 0 {
                    break;
                }
                let name = read_string(reader)?;
                let val = read_payload(reader, tag_type)?;
                map.insert(name, val);
            }
            Ok(NbtTag::Compound(NbtCompound { map }))
        }
        _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("NBT readTag: unknown tag type {tag_type}"))),
    }
}

pub fn write_payload<W: Write>(writer: &mut W, tag: &NbtTag) -> std::io::Result<()> {
    match tag {
        NbtTag::End => Ok(()),
        NbtTag::Byte(val) => writer.write_all(&[*val as u8]),
        NbtTag::Short(val) => writer.write_all(&val.to_be_bytes()),
        NbtTag::Int(val) => writer.write_all(&val.to_be_bytes()),
        NbtTag::Long(val) => writer.write_all(&val.to_be_bytes()),
        NbtTag::Float(val) => writer.write_all(&val.to_be_bytes()),
        NbtTag::Double(val) => writer.write_all(&val.to_be_bytes()),
        NbtTag::ByteArray(bytes) => {
            let len = bytes.len() as i32;
            writer.write_all(&len.to_be_bytes())?;
            writer.write_all(bytes)
        }
        NbtTag::String(s) => write_string(writer, s),
        NbtTag::List(list) => {
            let actual = if list.elements.is_empty() { 1 } else { list.tag_type };
            writer.write_all(&[actual])?;
            let len = list.elements.len() as i32;
            writer.write_all(&len.to_be_bytes())?;
            for elem in &list.elements {
                write_payload(writer, elem)?;
            }
            Ok(())
        }
        NbtTag::Compound(comp) => {
            for (name, tag) in &comp.map {
                let tag_type = tag.tag_type();
                writer.write_all(&[tag_type])?;
                write_string(writer, name)?;
                write_payload(writer, tag)?;
            }
            writer.write_all(&[0]) // TAG_End
        }
    }
}

pub fn read_root<R: Read>(reader: &mut R) -> std::io::Result<(String, NbtCompound)> {
    let mut type_buf = [0u8; 1];
    reader.read_exact(&mut type_buf)?;
    if type_buf[0] != 10 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "root must be TAG_Compound"));
    }
    let name = read_string(reader)?;
    let tag = read_payload(reader, 10)?;
    if let NbtTag::Compound(comp) = tag {
        Ok((name, comp))
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "expected NbtCompound"))
    }
}

pub fn write_root<W: Write>(writer: &mut W, name: &str, comp: &NbtCompound) -> std::io::Result<()> {
    writer.write_all(&[10])?;
    write_string(writer, name)?;
    write_payload(writer, &NbtTag::Compound(comp.clone()))
}

fn read_nbt_string_from_buffer(buf: &mut ByteBuffer) -> Result<String, NbtError> {
    let raw = buf.read_short().map_err(NbtError::from)?;
    if raw < 0 {
        return Err(NbtError::NegativeStringLength(raw));
    }
    if raw == 0 {
        return Ok(String::new());
    }
    match usize::try_from(raw) {
        Ok(n) => {
            let bytes = buf.read_bytes(n).map_err(NbtError::from)?;
            match String::from_utf8(bytes) {
                Ok(s) => Ok(s),
                Err(e) => Err(NbtError::InvalidUtf8(e.to_string())),
            }
        }
        Err(_) => Err(NbtError::NegativeStringLength(raw)),
    }
}

fn write_nbt_string_to_buffer(buf: &mut ByteBuffer, s: &str) -> Result<(), NbtError> {
    match i16::try_from(s.len()) {
        Ok(n) => {
            buf.write_short(n);
            buf.write_bytes(s.as_bytes());
            Ok(())
        }
        Err(_) => Err(NbtError::StringTooLong(s.len())),
    }
}

pub fn read_payload_from_buffer(buf: &mut ByteBuffer, tag_type: u8) -> Result<NbtTag, NbtError> {
    match tag_type {
        0 => Ok(NbtTag::End),
        1 => {
            let v = buf.read_byte().map_err(NbtError::from)?;
            Ok(NbtTag::Byte(v))
        }
        2 => {
            let v = buf.read_short().map_err(NbtError::from)?;
            Ok(NbtTag::Short(v))
        }
        3 => {
            let v = buf.read_int().map_err(NbtError::from)?;
            Ok(NbtTag::Int(v))
        }
        4 => {
            let v = buf.read_long().map_err(NbtError::from)?;
            Ok(NbtTag::Long(v))
        }
        5 => {
            let v = buf.read_float().map_err(NbtError::from)?;
            Ok(NbtTag::Float(v))
        }
        6 => {
            let v = buf.read_double().map_err(NbtError::from)?;
            Ok(NbtTag::Double(v))
        }
        7 => {
            let len = buf.read_int().map_err(NbtError::from)?;
            if len < 0 {
                return Err(NbtError::NegativeByteArrayLength(len));
            }
            if len == 0 {
                return Ok(NbtTag::ByteArray(Vec::new()));
            }
            match usize::try_from(len) {
                Ok(n) => {
                    let bytes = buf.read_bytes(n).map_err(NbtError::from)?;
                    Ok(NbtTag::ByteArray(bytes))
                }
                Err(_) => Err(NbtError::NegativeByteArrayLength(len)),
            }
        }
        8 => {
            let s = read_nbt_string_from_buffer(buf)?;
            Ok(NbtTag::String(s))
        }
        9 => {
            let elem_type = buf.read_ubyte().map_err(NbtError::from)?;
            match NbtTagType::from_u8(elem_type) {
                Some(_) => {},
                None => return Err(NbtError::UnknownTagType(elem_type)),
            }
            let count = buf.read_int().map_err(NbtError::from)?;
            if count < 0 {
                return Err(NbtError::NegativeListLength(count));
            }
            if count == 0 {
                return Ok(NbtTag::List(NbtList {
                    tag_type: elem_type,
                    elements: Vec::new(),
                }));
            }
            match usize::try_from(count) {
                Ok(n) => {
                    let mut elements = Vec::with_capacity(n.min(1024 * 1024));
                    for _ in 0..n {
                        elements.push(read_payload_from_buffer(buf, elem_type)?);
                    }
                    Ok(NbtTag::List(NbtList {
                        tag_type: elem_type,
                        elements,
                    }))
                }
                Err(_) => Err(NbtError::NegativeListLength(count)),
            }
        }
        10 => {
            let mut map = BTreeMap::new();
            loop {
                if buf.remaining() == 0 {
                    break;
                }
                let t = buf.read_ubyte().map_err(NbtError::from)?;
                if t == 0 {
                    break;
                }
                match NbtTagType::from_u8(t) {
                    Some(_) => {},
                    None => return Err(NbtError::UnknownTagType(t)),
                }
                let name = read_nbt_string_from_buffer(buf)?;
                let val = read_payload_from_buffer(buf, t)?;
                map.insert(name, val);
            }
            Ok(NbtTag::Compound(NbtCompound { map }))
        }
        _ => Err(NbtError::UnknownTagType(tag_type)),
    }
}

pub fn write_payload_to_buffer(buf: &mut ByteBuffer, tag: &NbtTag) -> Result<(), NbtError> {
    match tag {
        NbtTag::End => Ok(()),
        NbtTag::Byte(v) => {
            buf.write_byte(*v);
            Ok(())
        }
        NbtTag::Short(v) => {
            buf.write_short(*v);
            Ok(())
        }
        NbtTag::Int(v) => {
            buf.write_int(*v);
            Ok(())
        }
        NbtTag::Long(v) => {
            buf.write_long(*v);
            Ok(())
        }
        NbtTag::Float(v) => {
            buf.write_float(*v);
            Ok(())
        }
        NbtTag::Double(v) => {
            buf.write_double(*v);
            Ok(())
        }
        NbtTag::ByteArray(bytes) => match i32::try_from(bytes.len()) {
            Ok(n) => {
                buf.write_int(n);
                buf.write_bytes(bytes);
                Ok(())
            }
            Err(_) => Err(NbtError::ArrayTooLong(bytes.len())),
        },
        NbtTag::String(s) => write_nbt_string_to_buffer(buf, s),
        NbtTag::List(list) => {
            let actual = if list.elements.is_empty() {
                1
            } else {
                list.tag_type
            };
            buf.write_ubyte(actual);
            match i32::try_from(list.elements.len()) {
                Ok(n) => {
                    buf.write_int(n);
                    for elem in &list.elements {
                        write_payload_to_buffer(buf, elem)?;
                    }
                    Ok(())
                }
                Err(_) => Err(NbtError::ArrayTooLong(list.elements.len())),
            }
        }
        NbtTag::Compound(comp) => {
            for (name, child) in &comp.map {
                buf.write_ubyte(child.tag_type());
                write_nbt_string_to_buffer(buf, name)?;
                write_payload_to_buffer(buf, child)?;
            }
            buf.write_ubyte(0);
            Ok(())
        }
    }
}

pub fn read_root_from_buffer(buf: &mut ByteBuffer) -> Result<(String, NbtCompound), NbtError> {
    if buf.remaining() == 0 {
        return Err(NbtError::InvalidRoot("empty buffer".to_string()));
    }
    let tag_type = buf.read_ubyte().map_err(NbtError::from)?;
    if tag_type != 10 {
        return Err(NbtError::InvalidRoot(format!(
            "root must be TAG_Compound, got {tag_type}"
        )));
    }
    let name = read_nbt_string_from_buffer(buf)?;
    let tag = read_payload_from_buffer(buf, 10)?;
    match tag {
        NbtTag::Compound(comp) => Ok((name, comp)),
        _ => Err(NbtError::InvalidRoot("expected NbtCompound".to_string())),
    }
}

pub fn write_root_to_buffer(
    buf: &mut ByteBuffer,
    name: &str,
    comp: &NbtCompound,
) -> Result<(), NbtError> {
    buf.write_ubyte(10);
    write_nbt_string_to_buffer(buf, name)?;
    write_payload_to_buffer(buf, &NbtTag::Compound(comp.clone()))
}

pub fn encode_root(name: &str, comp: &NbtCompound) -> Result<Vec<u8>, NbtError> {
    let mut out = Vec::new();
    write_root(&mut out, name, comp).map_err(NbtError::from)?;
    Ok(out)
}

pub fn decode_root(bytes: &[u8]) -> Result<(String, NbtCompound), NbtError> {
    let mut cursor = std::io::Cursor::new(bytes);
    read_root(&mut cursor).map_err(NbtError::from)
}

pub fn write_gzip_root(name: &str, comp: &NbtCompound) -> Result<Vec<u8>, NbtError> {
    let raw = encode_root(name, comp)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).map_err(NbtError::from)?;
    encoder.finish().map_err(NbtError::from)
}

pub fn read_gzip_root(data: &[u8]) -> Result<(String, NbtCompound), NbtError> {
    if data.is_empty() {
        return Err(NbtError::InvalidRoot("empty gzip input".to_string()));
    }
    let mut decoder = GzDecoder::new(data);
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).map_err(NbtError::from)?;
    if raw.is_empty() {
        return Err(NbtError::InvalidRoot("empty decompressed NBT".to_string()));
    }
    decode_root(&raw)
}

pub fn read_gzip_root_with_fallback(data: &[u8]) -> Result<(String, NbtCompound), NbtError> {
    match read_gzip_root(data) {
        Ok(v) => Ok(v),
        Err(_) => decode_root(data),
    }
}

pub fn write_gzip_file(path: &Path, name: &str, comp: &NbtCompound) -> Result<(), NbtError> {
    let compressed = write_gzip_root(name, comp)?;
    std::fs::write(path, &compressed).map_err(NbtError::from)?;
    Ok(())
}

pub fn read_gzip_file(path: &Path) -> Result<(String, NbtCompound), NbtError> {
    let bytes = std::fs::read(path).map_err(NbtError::from)?;
    read_gzip_root(&bytes)
}

pub fn read_nbt_file_with_fallback(path: &Path) -> Result<(String, NbtCompound), NbtError> {
    let bytes = std::fs::read(path).map_err(NbtError::from)?;
    read_gzip_root_with_fallback(&bytes)
}

#[cfg(test)]
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;

    fn payload_roundtrip(tag: NbtTag, tag_type: u8) -> NbtTag {
        let mut out = Vec::new();
        write_payload(&mut out, &tag).unwrap();
        let mut cursor = std::io::Cursor::new(out);
        read_payload(&mut cursor, tag_type).unwrap()
    }

    fn buffer_roundtrip(tag: NbtTag, tag_type: u8) -> NbtTag {
        let mut buf = ByteBuffer::new();
        write_payload_to_buffer(&mut buf, &tag).unwrap();
        buf.read_pos = 0;
        read_payload_from_buffer(&mut buf, tag_type).unwrap()
    }

    #[test]
    fn test_compound_roundtrip() {
        let mut comp = NbtCompound { map: BTreeMap::new() };
        comp.map.insert("byteVal".to_string(), NbtTag::Byte(127));
        comp.map.insert("shortVal".to_string(), NbtTag::Short(-30000));
        comp.map.insert("intVal".to_string(), NbtTag::Int(999999));
        comp.map.insert("strVal".to_string(), NbtTag::String("hello".to_string()));
        comp.map.insert("floatVal".to_string(), NbtTag::Float(1.5));
        comp.map.insert("doubleVal".to_string(), NbtTag::Double(2.5));

        let mut out = Vec::new();
        write_root(&mut out, "Root", &comp).unwrap();

        let mut cursor = std::io::Cursor::new(out);
        let (name, read_back) = read_root(&mut cursor).unwrap();

        assert_eq!(name, "Root");
        assert_eq!(read_back, comp);
    }

    #[test]
    fn test_byte_roundtrip() {
        let back = payload_roundtrip(NbtTag::Byte(42), 1);
        assert_eq!(back, NbtTag::Byte(42));
        let back2 = buffer_roundtrip(NbtTag::Byte(-128), 1);
        assert_eq!(back2, NbtTag::Byte(-128));
    }

    #[test]
    fn test_short_roundtrip() {
        let back = payload_roundtrip(NbtTag::Short(-32000), 2);
        assert_eq!(back, NbtTag::Short(-32000));
    }

    #[test]
    fn test_int_roundtrip() {
        let back = payload_roundtrip(NbtTag::Int(1234567890), 3);
        assert_eq!(back, NbtTag::Int(1234567890));
    }

    #[test]
    fn test_long_roundtrip() {
        let val: i64 = 987654321012345678;
        let back = payload_roundtrip(NbtTag::Long(val), 4);
        assert_eq!(back, NbtTag::Long(val));
        let back2 = buffer_roundtrip(NbtTag::Long(val), 4);
        assert_eq!(back2, NbtTag::Long(val));
        let back3 = buffer_roundtrip(NbtTag::Long(i64::MIN), 4);
        assert_eq!(back3, NbtTag::Long(i64::MIN));
    }

    #[test]
    fn test_float_roundtrip() {
        let back = payload_roundtrip(NbtTag::Float(3.14159), 5);
        match back {
            NbtTag::Float(v) => assert!((v - 3.14159).abs() < 1e-6),
            _ => panic!("wrong tag"),
        }
    }

    #[test]
    fn test_double_roundtrip() {
        let back = payload_roundtrip(NbtTag::Double(2.718281828459045), 6);
        match back {
            NbtTag::Double(v) => assert!((v - 2.718281828459045).abs() < 1e-12),
            _ => panic!("wrong tag"),
        }
    }

    #[test]
    fn test_string_roundtrip() {
        let back = payload_roundtrip(NbtTag::String("Hello NBT!".to_string()), 8);
        assert_eq!(back, NbtTag::String("Hello NBT!".to_string()));
    }

    #[test]
    fn test_empty_string_roundtrip() {
        let back = payload_roundtrip(NbtTag::String(String::new()), 8);
        assert_eq!(back, NbtTag::String(String::new()));
    }

    #[test]
    fn test_byte_array_roundtrip() {
        let d = vec![1u8, 2, 3, 255, 0, 128];
        let back = payload_roundtrip(NbtTag::ByteArray(d.clone()), 7);
        assert_eq!(back, NbtTag::ByteArray(d.clone()));
        let back2 = buffer_roundtrip(NbtTag::ByteArray(d.clone()), 7);
        assert_eq!(back2, NbtTag::ByteArray(d));
    }

    #[test]
    fn test_compound_missing_key() {
        let comp = NbtCompound::new();
        assert_eq!(comp.get_int("nonexistent"), 0);
        assert_eq!(comp.get_string("nonexistent"), "");
        assert_eq!(comp.get_byte("nonexistent"), 0);
        assert_eq!(comp.get_short("nonexistent"), 0);
        assert_eq!(comp.get_long("nonexistent"), 0);
        assert!(comp.get_compound("nonexistent").is_none());
        assert!(comp.get_list("nonexistent").is_none());
        assert!(comp.is_empty());
        assert_eq!(comp.len(), 0);
    }

    #[test]
    fn test_list_roundtrip() {
        let mut list = NbtList::with_type(3);
        list.push(NbtTag::Int(10));
        list.push(NbtTag::Int(20));
        list.push(NbtTag::Int(30));
        let back = payload_roundtrip(NbtTag::List(list), 9);
        match back {
            NbtTag::List(l) => {
                assert_eq!(l.elements.len(), 3);
                assert_eq!(l.elements[0], NbtTag::Int(10));
                assert_eq!(l.elements[2], NbtTag::Int(30));
            }
            _ => panic!("wrong tag"),
        }
    }

    #[test]
    fn test_empty_list_roundtrip() {
        let list = NbtList::new();
        let back = payload_roundtrip(NbtTag::List(list), 9);
        match back {
            NbtTag::List(l) => assert!(l.is_empty()),
            _ => panic!("wrong tag"),
        }
        let mut buf = ByteBuffer::new();
        write_payload_to_buffer(&mut buf, &NbtTag::List(NbtList::new())).unwrap();
        buf.read_pos = 0;
        let back2 = read_payload_from_buffer(&mut buf, 9).unwrap();
        match back2 {
            NbtTag::List(l) => assert!(l.is_empty()),
            _ => panic!("wrong tag"),
        }
    }

    #[test]
    fn test_nested_compound() {
        let mut inner = NbtCompound::new();
        inner.set_string("name", "inner");
        inner.set_int("value", 42);
        let mut outer = NbtCompound::new();
        outer.set_compound("child", Some(inner));
        let mut out = Vec::new();
        write_root(&mut out, "Root", &outer).unwrap();
        let mut cursor = std::io::Cursor::new(out);
        let (_, read_back) = read_root(&mut cursor).unwrap();
        let child = read_back.get_compound("child").unwrap();
        assert_eq!(child.get_string("name"), "inner");
        assert_eq!(child.get_int("value"), 42);
    }

    #[test]
    fn test_compound_set_boolean() {
        let mut comp = NbtCompound::new();
        comp.set_boolean("flag", true);
        comp.set_boolean("noflag", false);
        assert_eq!(comp.get_byte("flag"), 1);
        assert_eq!(comp.get_byte("noflag"), 0);
        assert!(comp.get_boolean("flag"));
        assert!(!comp.get_boolean("noflag"));
    }

    #[test]
    fn test_compound_set_byte_array() {
        let mut comp = NbtCompound::new();
        let d = vec![0xDEu8, 0xAD, 0xBE, 0xEF];
        comp.set_byte_array("data", d.clone());
        let mut out = Vec::new();
        write_root(&mut out, "", &comp).unwrap();
        let mut cursor = std::io::Cursor::new(out);
        let (_, read_back) = read_root(&mut cursor).unwrap();
        assert_eq!(read_back.get_byte_array("data"), d);
    }

    #[test]
    fn test_list_of_compounds() {
        let mut list = NbtList::with_type(10);
        for i in 0..3 {
            let mut c = NbtCompound::new();
            c.set_int("index", i);
            list.push(NbtTag::Compound(c));
        }
        let back = payload_roundtrip(NbtTag::List(list), 9);
        match back {
            NbtTag::List(l) => {
                assert_eq!(l.elements.len(), 3);
                for (i, elem) in l.elements.iter().enumerate() {
                    match elem {
                        NbtTag::Compound(c) => assert_eq!(c.get_int("index"), i as i32),
                        _ => panic!("wrong elem"),
                    }
                }
            }
            _ => panic!("wrong tag"),
        }
    }

    #[test]
    fn test_compound_map_find() {
        let mut comp = NbtCompound::new();
        comp.set_string("Items", "some_value");
        let found = comp.map.get("Items").unwrap();
        assert_eq!(found, &NbtTag::String("some_value".to_string()));
        assert!(comp.contains("Items"));
        assert_eq!(comp.len(), 1);
        assert!(!comp.is_empty());
    }

    #[test]
    fn test_compound_long_helpers() {
        let mut comp = NbtCompound::new();
        comp.set_long("seed", 987654321012345678);
        comp.set_byte("b", -5);
        comp.set_short("s", -300);
        comp.set_int("i", 123456);
        comp.set_float("f", 1.5);
        comp.set_double("d", 2.5);
        comp.set_string("t", "hi");
        assert_eq!(comp.get_long("seed"), 987654321012345678);
        assert_eq!(comp.get_byte("b"), -5);
        assert_eq!(comp.get_short("s"), -300);
        assert_eq!(comp.get_int("i"), 123456);
        assert!((comp.get_float("f") - 1.5).abs() < 1e-6);
        assert!((comp.get_double("d") - 2.5).abs() < 1e-12);
        assert_eq!(comp.get_string("t"), "hi");
        comp.set_compound("gone", None);
        assert!(!comp.contains("gone"));
        comp.set_tag("tmp", Some(NbtTag::Int(1)));
        assert!(comp.contains("tmp"));
        comp.set_tag("tmp", None);
        assert!(!comp.contains("tmp"));
    }

    #[test]
    fn test_unknown_tag_type_error() {
        let mut cur = std::io::Cursor::new(vec![]);
        assert!(read_payload(&mut cur, 42).is_err());
        let mut buf = ByteBuffer::new();
        assert!(read_payload_from_buffer(&mut buf, 42).is_err());
        assert!(NbtTagType::from_u8(42).is_none());
        assert_eq!(tag_type_name(42), "TAG_Unknown");
        assert_eq!(tag_type_name(4), "TAG_Long");
    }

    #[test]
    fn test_negative_lengths_error() {
        let mut cur = std::io::Cursor::new(vec![0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(read_payload(&mut cur, 7).is_err());
        let mut cur2 = std::io::Cursor::new(vec![1, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(read_payload(&mut cur2, 9).is_err());
        let mut cur3 = std::io::Cursor::new(vec![0xFF, 0xFF]);
        assert!(read_string(&mut cur3).is_err());
        let mut buf = ByteBuffer::from_vec(vec![0xFF, 0xFF, 0xFF, 0xFE]);
        assert!(read_payload_from_buffer(&mut buf, 7).is_err());
    }

    #[test]
    fn test_nbt_error_display() {
        let e = NbtError::UnknownTagType(99);
        assert!(e.to_string().contains("99"));
        let e2 = NbtError::NegativeByteArrayLength(-1);
        assert!(e2.to_string().contains("-1"));
        let e3 = NbtError::InvalidRoot("x".to_string());
        assert!(e3.to_string().contains('x'));
        let io_e: std::io::Error = e.into();
        assert_eq!(io_e.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_gzip_roundtrip_and_magic() {
        let mut comp = NbtCompound::new();
        comp.set_long("RandomSeed", 12345);
        comp.set_int("SpawnX", 5);
        comp.set_string("LevelName", "world");
        let compressed = write_gzip_root("", &comp).unwrap();
        assert!(compressed.len() > 2);
        assert_eq!(compressed[0], 0x1F);
        assert_eq!(compressed[1], 0x8B);
        assert!(is_gzip_magic(&compressed));
        assert!(!is_gzip_magic(&[]));
        let (name, back) = read_gzip_root(&compressed).unwrap();
        assert_eq!(name, "");
        assert_eq!(back.get_long("RandomSeed"), 12345);
        assert_eq!(back.get_int("SpawnX"), 5);
        assert_eq!(back.get_string("LevelName"), "world");
    }

    #[test]
    fn test_gzip_fallback_raw() {
        let mut comp = NbtCompound::new();
        comp.set_int("version", 19132);
        let raw = encode_root("", &comp).unwrap();
        assert!(!is_gzip_magic(&raw));
        let (_, back) = read_gzip_root_with_fallback(&raw).unwrap();
        assert_eq!(back.get_int("version"), 19132);
        let (_, back2) = decode_root(&raw).unwrap();
        assert_eq!(back2.get_int("version"), 19132);
    }

    #[test]
    fn test_gzip_file_roundtrip() {
        let mut comp = NbtCompound::new();
        comp.set_string("LevelName", "test");
        comp.set_long("Time", 999);
        let path = std::path::PathBuf::from("/tmp/nbt_gzip_test.dat");
        write_gzip_file(&path, "", &comp).unwrap();
        let (_, back) = read_gzip_file(&path).unwrap();
        assert_eq!(back.get_string("LevelName"), "test");
        assert_eq!(back.get_long("Time"), 999);
        let (_, back2) = read_nbt_file_with_fallback(&path).unwrap();
        assert_eq!(back2.get_long("Time"), 999);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_inventory_list_serialization() {
        let mut list = NbtList::with_type(10);
        let mut c0 = NbtCompound::new();
        c0.set_short("id", 264);
        c0.set_byte("Count", 32);
        c0.set_short("Damage", 5);
        c0.set_byte("Slot", 0);
        let mut c1 = NbtCompound::new();
        c1.set_short("id", 263);
        c1.set_byte("Count", 64);
        c1.set_short("Damage", 0);
        c1.set_byte("Slot", 1);
        list.push(NbtTag::Compound(c0));
        list.push(NbtTag::Compound(c1));
        assert_eq!(list.tag_type, 10);
        assert_eq!(list.len(), 2);
        let back = payload_roundtrip(NbtTag::List(list), 9);
        match back {
            NbtTag::List(l) => {
                assert_eq!(l.len(), 2);
                match &l.elements[0] {
                    NbtTag::Compound(c) => {
                        assert_eq!(c.get_short("id"), 264);
                        assert_eq!(c.get_byte("Count"), 32);
                        assert_eq!(c.get_short("Damage"), 5);
                    }
                    _ => panic!("wrong elem"),
                }
                match &l.elements[1] {
                    NbtTag::Compound(c) => {
                        assert_eq!(c.get_short("id"), 263);
                        assert_eq!(c.get_byte("Count"), 64);
                    }
                    _ => panic!("wrong elem"),
                }
            }
            _ => panic!("wrong tag"),
        }
    }

    #[test]
    fn test_player_like_gzip_roundtrip() {
        let mut comp = NbtCompound::new();
        comp.set_double("posX", 128.5);
        comp.set_double("posY", 65.0);
        comp.set_double("posZ", -256.25);
        comp.set_float("yaw", 90.0);
        comp.set_short("health", 18);
        comp.set_int("score", 420);
        let mut inv = NbtList::with_type(10);
        let mut item = NbtCompound::new();
        item.set_short("id", 276);
        item.set_byte("Count", 1);
        item.set_short("Damage", 5);
        inv.push(NbtTag::Compound(item));
        comp.set_list("Inventory", Some(inv));
        let compressed = write_gzip_root("Player", &comp).unwrap();
        assert!(is_gzip_magic(&compressed));
        let (name, back) = read_gzip_root(&compressed).unwrap();
        assert_eq!(name, "Player");
        assert!((back.get_double("posX") - 128.5).abs() < 1e-9);
        assert_eq!(back.get_short("health"), 18);
        assert_eq!(back.get_int("score"), 420);
        let inv_back = back.get_list("Inventory").unwrap();
        assert_eq!(inv_back.len(), 1);
    }

    #[test]
    fn test_bytebuffer_root_integration() {
        let mut comp = NbtCompound::new();
        comp.set_long("Time", -9999999999);
        comp.set_int("SpawnY", 64);
        comp.set_string("LevelName", "hello");
        let mut inner = NbtCompound::new();
        inner.set_int("v", 7);
        comp.set_compound("child", Some(inner));
        let mut list = NbtList::with_type(3);
        list.push(NbtTag::Int(1));
        list.push(NbtTag::Int(2));
        comp.set_list("nums", Some(list));
        let mut buf = ByteBuffer::new();
        write_root_to_buffer(&mut buf, "Data", &comp).unwrap();
        buf.read_pos = 0;
        let (name, back) = read_root_from_buffer(&mut buf).unwrap();
        assert_eq!(name, "Data");
        assert_eq!(back.get_long("Time"), -9999999999);
        assert_eq!(back.get_int("SpawnY"), 64);
        assert_eq!(back.get_string("LevelName"), "hello");
        assert_eq!(back.get_compound("child").unwrap().get_int("v"), 7);
        assert_eq!(back.get_list("nums").unwrap().len(), 2);
    }

    #[test]
    fn test_empty_buffer_root_fails() {
        let mut buf = ByteBuffer::new();
        assert!(read_root_from_buffer(&mut buf).is_err());
        let mut buf2 = ByteBuffer::from_vec(vec![1, 0, 0]);
        assert!(read_root_from_buffer(&mut buf2).is_err());
    }
}

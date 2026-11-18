use serde_json::{Map, Number, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum PlistValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Real(f64),
    String(String),
    Data(Vec<u8>),
    Array(Vec<PlistValue>),
    Dict(BTreeMap<String, PlistValue>),
}

impl PlistValue {
    pub fn dict(entries: impl IntoIterator<Item = (impl Into<String>, PlistValue)>) -> Self {
        let mut map = BTreeMap::new();
        for (k, v) in entries {
            map.insert(k.into(), v);
        }
        PlistValue::Dict(map)
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<String, PlistValue>> {
        match self {
            PlistValue::Dict(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[PlistValue]> {
        match self {
            PlistValue::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            PlistValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            PlistValue::Int(v) => Some(*v),
            PlistValue::UInt(v) => i64::try_from(*v).ok(),
            PlistValue::Real(v) => Some(*v as i64),
            PlistValue::Bool(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            PlistValue::UInt(v) => Some(*v),
            PlistValue::Int(v) if *v >= 0 => Some(*v as u64),
            PlistValue::Real(v) if *v >= 0.0 => Some(*v as u64),
            PlistValue::Bool(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            PlistValue::Real(v) => Some(*v),
            PlistValue::Int(v) => Some(*v as f64),
            PlistValue::UInt(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PlistValue::Bool(b) => Some(*b),
            PlistValue::Int(v) => Some(*v != 0),
            PlistValue::UInt(v) => Some(*v != 0),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&PlistValue> {
        self.as_dict()?.get(key)
    }

    pub fn to_json(&self) -> Value {
        match self {
            PlistValue::Null => Value::Null,
            PlistValue::Bool(b) => Value::Bool(*b),
            PlistValue::Int(v) => Value::Number((*v).into()),
            PlistValue::UInt(v) => Value::Number((*v).into()),
            PlistValue::Real(v) => Number::from_f64(*v)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            PlistValue::String(s) => Value::String(s.clone()),
            PlistValue::Data(d) => Value::String(bytes_to_upper_hex(d)),
            PlistValue::Array(arr) => Value::Array(arr.iter().map(PlistValue::to_json).collect()),
            PlistValue::Dict(map) => {
                let mut out = Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), v.to_json());
                }
                Value::Object(out)
            }
        }
    }
}

pub fn bytes_to_upper_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

pub fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = hex.trim();
    if trimmed.len() % 2 != 0 {
        return Err(format!("Hex 字符串长度不是偶数: {}", trimmed.len()));
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let raw = trimmed.as_bytes();
    for i in (0..raw.len()).step_by(2) {
        let hi = hex_nibble(raw[i])?;
        let lo = hex_nibble(raw[i + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("非法 Hex 字符: 0x{b:02X}")),
    }
}

enum FlatNode {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Real(f64),
    String(String),
    Data(Vec<u8>),
    Array(Vec<usize>),
    Dict(Vec<(usize, usize)>),
}

fn flatten_value(val: &PlistValue, nodes: &mut Vec<FlatNode>) -> usize {
    let idx = nodes.len();
    // Reserve slot so parent comes before or at deterministic index (top_object = 0)
    nodes.push(FlatNode::Null);

    let built = match val {
        PlistValue::Null => FlatNode::Null,
        PlistValue::Bool(b) => FlatNode::Bool(*b),
        PlistValue::Int(i) => FlatNode::Int(*i),
        PlistValue::UInt(u) => FlatNode::UInt(*u),
        PlistValue::Real(r) => FlatNode::Real(*r),
        PlistValue::String(s) => FlatNode::String(s.clone()),
        PlistValue::Data(d) => FlatNode::Data(d.clone()),
        PlistValue::Array(items) => {
            let mut refs = Vec::with_capacity(items.len());
            for item in items {
                refs.push(flatten_value(item, nodes));
            }
            FlatNode::Array(refs)
        }
        PlistValue::Dict(map) => {
            let mut pairs = Vec::with_capacity(map.len());
            for (k, v) in map {
                let k_val = PlistValue::String(k.clone());
                let k_idx = flatten_value(&k_val, nodes);
                let v_idx = flatten_value(v, nodes);
                pairs.push((k_idx, v_idx));
            }
            FlatNode::Dict(pairs)
        }
    };

    nodes[idx] = built;
    idx
}

fn min_uint_bytes(v: usize) -> u8 {
    if v <= 0xFF {
        1
    } else if v <= 0xFFFF {
        2
    } else if v <= 0xFFFF_FFFF {
        4
    } else {
        8
    }
}

fn write_be_uint(buf: &mut Vec<u8>, val: u64, width: u8) {
    match width {
        1 => buf.push(val as u8),
        2 => buf.extend_from_slice(&(val as u16).to_be_bytes()),
        4 => buf.extend_from_slice(&(val as u32).to_be_bytes()),
        8 => buf.extend_from_slice(&val.to_be_bytes()),
        _ => unreachable!(),
    }
}

fn write_int_object(buf: &mut Vec<u8>, val: i64) {
    if val < 0 {
        buf.push(0x13);
        buf.extend_from_slice(&val.to_be_bytes());
    } else {
        write_uint_object(buf, val as u64);
    }
}

fn write_uint_object(buf: &mut Vec<u8>, val: u64) {
    if val <= 0xFF {
        buf.push(0x10);
        buf.push(val as u8);
    } else if val <= 0xFFFF {
        buf.push(0x11);
        buf.extend_from_slice(&(val as u16).to_be_bytes());
    } else if val <= 0xFFFF_FFFF {
        buf.push(0x12);
        buf.extend_from_slice(&(val as u32).to_be_bytes());
    } else if val <= i64::MAX as u64 {
        buf.push(0x13);
        buf.extend_from_slice(&val.to_be_bytes());
    } else {
        buf.push(0x14);
        buf.extend_from_slice(&0u64.to_be_bytes());
        buf.extend_from_slice(&val.to_be_bytes());
    }
}

fn write_type_and_len(buf: &mut Vec<u8>, high_nibble: u8, len: usize) {
    if len < 0x0F {
        buf.push((high_nibble << 4) | (len as u8));
    } else {
        buf.push((high_nibble << 4) | 0x0F);
        write_uint_object(buf, len as u64);
    }
}

pub fn encode_bplist(root: &PlistValue) -> Vec<u8> {
    let mut nodes = Vec::new();
    let top_idx = flatten_value(root, &mut nodes);
    let num_objects = nodes.len();
    let obj_ref_size = min_uint_bytes(num_objects.saturating_sub(1));

    let mut buf = Vec::with_capacity(128 + num_objects * 16);
    buf.extend_from_slice(b"bplist00");

    let mut offsets = Vec::with_capacity(num_objects);
    for node in &nodes {
        offsets.push(buf.len());
        match node {
            FlatNode::Null => buf.push(0x00),
            FlatNode::Bool(false) => buf.push(0x08),
            FlatNode::Bool(true) => buf.push(0x09),
            FlatNode::Int(v) => write_int_object(&mut buf, *v),
            FlatNode::UInt(v) => write_uint_object(&mut buf, *v),
            FlatNode::Real(v) => {
                buf.push(0x23);
                buf.extend_from_slice(&v.to_bits().to_be_bytes());
            }
            FlatNode::String(s) => {
                if s.is_ascii() {
                    write_type_and_len(&mut buf, 0x5, s.len());
                    buf.extend_from_slice(s.as_bytes());
                } else {
                    let u16s: Vec<u16> = s.encode_utf16().collect();
                    write_type_and_len(&mut buf, 0x6, u16s.len());
                    for unit in u16s {
                        buf.extend_from_slice(&unit.to_be_bytes());
                    }
                }
            }
            FlatNode::Data(d) => {
                write_type_and_len(&mut buf, 0x4, d.len());
                buf.extend_from_slice(d);
            }
            FlatNode::Array(refs) => {
                write_type_and_len(&mut buf, 0xA, refs.len());
                for &r in refs {
                    write_be_uint(&mut buf, r as u64, obj_ref_size);
                }
            }
            FlatNode::Dict(pairs) => {
                write_type_and_len(&mut buf, 0xD, pairs.len());
                for &(k, _) in pairs {
                    write_be_uint(&mut buf, k as u64, obj_ref_size);
                }
                for &(_, v) in pairs {
                    write_be_uint(&mut buf, v as u64, obj_ref_size);
                }
            }
        }
    }

    let offset_table_offset = buf.len();
    let max_offset = offsets.last().copied().unwrap_or(8);
    let offset_int_size = min_uint_bytes(max_offset);

    for off in offsets {
        write_be_uint(&mut buf, off as u64, offset_int_size);
    }

    // 32-byte trailer
    buf.extend_from_slice(&[0u8; 6]);
    buf.push(offset_int_size);
    buf.push(obj_ref_size);
    buf.extend_from_slice(&(num_objects as u64).to_be_bytes());
    buf.extend_from_slice(&(top_idx as u64).to_be_bytes());
    buf.extend_from_slice(&(offset_table_offset as u64).to_be_bytes());

    buf
}

pub fn encode_bplist_hex(root: &PlistValue) -> String {
    bytes_to_upper_hex(&encode_bplist(root))
}

pub fn empty_dict_hex() -> String {
    encode_bplist_hex(&PlistValue::Dict(BTreeMap::new()))
}

fn read_be_uint(bytes: &[u8]) -> Result<u64, String> {
    if bytes.is_empty() || bytes.len() > 8 {
        return Err(format!("不支持的整数字节宽度: {}", bytes.len()));
    }
    let mut val = 0u64;
    for &b in bytes {
        val = (val << 8) | (b as u64);
    }
    Ok(val)
}

fn decode_len_and_cursor(data: &[u8], offset: usize, low_nibble: u8) -> Result<(usize, usize), String> {
    if low_nibble != 0x0F {
        return Ok((low_nibble as usize, offset + 1));
    }
    let next = *data
        .get(offset + 1)
        .ok_or_else(|| "bplist 长度字段越界".to_string())?;
    if (next >> 4) != 0x1 {
        return Err(format!("bplist 扩展长度标记非法: 0x{next:02X}"));
    }
    let int_width = 1usize << (next & 0x0F);
    let start = offset + 2;
    let end = start.checked_add(int_width).ok_or("bplist 长度溢出")?;
    let slice = data.get(start..end).ok_or("bplist 长度数据越界")?;
    let len = read_be_uint(slice)? as usize;
    Ok((len, end))
}

struct BplistDecoder<'a> {
    data: &'a [u8],
    offsets: Vec<usize>,
    obj_ref_size: usize,
}

impl<'a> BplistDecoder<'a> {
    fn read_ref(&self, pos: usize) -> Result<usize, String> {
        let end = pos.checked_add(self.obj_ref_size).ok_or("引用偏移溢出")?;
        let slice = self.data.get(pos..end).ok_or("对象引用越界")?;
        Ok(read_be_uint(slice)? as usize)
    }

    fn decode_at(&self, obj_idx: usize, depth: usize) -> Result<PlistValue, String> {
        if depth > 64 {
            return Err("bplist 嵌套层级过深".to_string());
        }
        let &off = self
            .offsets
            .get(obj_idx)
            .ok_or_else(|| format!("对象索引越界: {obj_idx}"))?;
        let marker = *self
            .data
            .get(off)
            .ok_or_else(|| format!("对象偏移越界: {off}"))?;

        let hi = marker >> 4;
        let lo = marker & 0x0F;

        match hi {
            0x0 => match lo {
                0x0 | 0xF => Ok(PlistValue::Null),
                0x8 => Ok(PlistValue::Bool(false)),
                0x9 => Ok(PlistValue::Bool(true)),
                _ => Err(format!("未知 bplist 单例标记: 0x{marker:02X}")),
            },
            0x1 => {
                let width = 1usize << lo;
                let start = off + 1;
                let end = start.checked_add(width).ok_or("整数偏移溢出")?;
                let slice = self.data.get(start..end).ok_or("整数内容越界")?;
                match width {
                    1 => Ok(PlistValue::Int(slice[0] as i64)),
                    2 => Ok(PlistValue::Int(u16::from_be_bytes([slice[0], slice[1]]) as i64)),
                    4 => Ok(PlistValue::Int(
                        u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) as i64,
                    )),
                    8 => {
                        let mut arr = [0u8; 8];
                        arr.copy_from_slice(slice);
                        Ok(PlistValue::Int(i64::from_be_bytes(arr)))
                    }
                    16 => {
                        let mut lo_arr = [0u8; 8];
                        lo_arr.copy_from_slice(&slice[8..16]);
                        Ok(PlistValue::UInt(u64::from_be_bytes(lo_arr)))
                    }
                    _ => Err(format!("不支持的 bplist 整数宽度: {width}")),
                }
            }
            0x2 => {
                let width = 1usize << lo;
                let start = off + 1;
                let end = start.checked_add(width).ok_or("浮点偏移溢出")?;
                let slice = self.data.get(start..end).ok_or("浮点内容越界")?;
                match width {
                    4 => {
                        let mut arr = [0u8; 4];
                        arr.copy_from_slice(slice);
                        Ok(PlistValue::Real(f32::from_bits(u32::from_be_bytes(arr)) as f64))
                    }
                    8 => {
                        let mut arr = [0u8; 8];
                        arr.copy_from_slice(slice);
                        Ok(PlistValue::Real(f64::from_bits(u64::from_be_bytes(arr))))
                    }
                    _ => Err(format!("不支持的 bplist 浮点宽度: {width}")),
                }
            }
            0x3 => {
                // CFDate (8-byte float seconds since 2001-01-01)
                let slice = self.data.get(off + 1..off + 9).ok_or("日期内容越界")?;
                let mut arr = [0u8; 8];
                arr.copy_from_slice(slice);
                Ok(PlistValue::Real(f64::from_bits(u64::from_be_bytes(arr))))
            }
            0x4 => {
                let (len, cur) = decode_len_and_cursor(self.data, off, lo)?;
                let end = cur.checked_add(len).ok_or("Data 长度溢出")?;
                let slice = self.data.get(cur..end).ok_or("Data 内容越界")?;
                Ok(PlistValue::Data(slice.to_vec()))
            }
            0x5 => {
                let (len, cur) = decode_len_and_cursor(self.data, off, lo)?;
                let end = cur.checked_add(len).ok_or("ASCII 字符串长度溢出")?;
                let slice = self.data.get(cur..end).ok_or("ASCII 字符串内容越界")?;
                let s: String = slice.iter().map(|&b| b as char).collect();
                Ok(PlistValue::String(s))
            }
            0x6 => {
                let (char_len, cur) = decode_len_and_cursor(self.data, off, lo)?;
                let byte_len = char_len.checked_mul(2).ok_or("UTF-16 字符串长度溢出")?;
                let end = cur.checked_add(byte_len).ok_or("UTF-16 字符串越界")?;
                let slice = self.data.get(cur..end).ok_or("UTF-16 字符串内容越界")?;
                let mut units = Vec::with_capacity(char_len);
                for chunk in slice.chunks_exact(2) {
                    units.push(u16::from_be_bytes([chunk[0], chunk[1]]));
                }
                Ok(PlistValue::String(String::from_utf16_lossy(&units)))
            }
            0x7 => {
                let (len, cur) = decode_len_and_cursor(self.data, off, lo)?;
                let end = cur.checked_add(len).ok_or("UTF-8 字符串长度溢出")?;
                let slice = self.data.get(cur..end).ok_or("UTF-8 字符串内容越界")?;
                Ok(PlistValue::String(String::from_utf8_lossy(slice).into_owned()))
            }
            0x8 => {
                let width = (lo as usize) + 1;
                let start = off + 1;
                let end = start.checked_add(width).ok_or("UID 偏移溢出")?;
                let slice = self.data.get(start..end).ok_or("UID 内容越界")?;
                Ok(PlistValue::UInt(read_be_uint(slice)?))
            }
            0xA | 0xC => {
                let (count, cur) = decode_len_and_cursor(self.data, off, lo)?;
                let mut out = Vec::with_capacity(count);
                for i in 0..count {
                    let ref_pos = cur + i * self.obj_ref_size;
                    let child_idx = self.read_ref(ref_pos)?;
                    out.push(self.decode_at(child_idx, depth + 1)?);
                }
                Ok(PlistValue::Array(out))
            }
            0xD => {
                let (count, cur) = decode_len_and_cursor(self.data, off, lo)?;
                let vals_base = cur + count * self.obj_ref_size;
                let mut map = BTreeMap::new();
                for i in 0..count {
                    let k_idx = self.read_ref(cur + i * self.obj_ref_size)?;
                    let v_idx = self.read_ref(vals_base + i * self.obj_ref_size)?;
                    let key_val = self.decode_at(k_idx, depth + 1)?;
                    let key_str = match key_val {
                        PlistValue::String(s) => s,
                        PlistValue::Int(n) => n.to_string(),
                        PlistValue::UInt(n) => n.to_string(),
                        other => format!("{other:?}"),
                    };
                    let val = self.decode_at(v_idx, depth + 1)?;
                    map.insert(key_str, val);
                }
                Ok(PlistValue::Dict(map))
            }
            _ => Err(format!("不支持的 bplist 对象类型标记: 0x{marker:02X}")),
        }
    }
}

pub fn decode_bplist(data: &[u8]) -> Result<PlistValue, String> {
    if data.len() < 40 {
        return Err(format!("bplist 数据过短: {} bytes", data.len()));
    }
    if &data[0..8] != b"bplist00" {
        return Err("不是有效的 bplist00 数据头".to_string());
    }

    let trailer = &data[data.len() - 32..];
    let offset_int_size = trailer[6] as usize;
    let obj_ref_size = trailer[7] as usize;
    if !(1..=8).contains(&offset_int_size) || !(1..=8).contains(&obj_ref_size) {
        return Err(format!(
            "bplist trailer 尺寸字段异常: offset_size={offset_int_size}, ref_size={obj_ref_size}"
        ));
    }

    let num_objects = u64::from_be_bytes(trailer[8..16].try_into().unwrap()) as usize;
    let top_object = u64::from_be_bytes(trailer[16..24].try_into().unwrap()) as usize;
    let offset_table_offset = u64::from_be_bytes(trailer[24..32].try_into().unwrap()) as usize;

    if num_objects == 0 {
        return Ok(PlistValue::Null);
    }

    let table_bytes = num_objects
        .checked_mul(offset_int_size)
        .ok_or("偏移表大小溢出")?;
    let table_end = offset_table_offset
        .checked_add(table_bytes)
        .ok_or("偏移表结束位置溢出")?;
    if table_end > data.len() - 32 {
        return Err("bplist 偏移表越界".to_string());
    }

    let mut offsets = Vec::with_capacity(num_objects);
    for i in 0..num_objects {
        let pos = offset_table_offset + i * offset_int_size;
        let off = read_be_uint(&data[pos..pos + offset_int_size])? as usize;
        offsets.push(off);
    }

    let decoder = BplistDecoder {
        data,
        offsets,
        obj_ref_size,
    };
    decoder.decode_at(top_object, 0)
}

pub fn decode_bplist_hex(hex: &str) -> Result<PlistValue, String> {
    let trimmed = hex.trim();
    if trimmed.is_empty() {
        return Ok(PlistValue::Null);
    }
    let bytes = hex_to_bytes(trimmed)?;
    decode_bplist(&bytes)
}

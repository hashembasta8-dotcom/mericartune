//! TunerPro XDF definition parser (real, tolerant, dependency-free).
//!
//! XDF is the industry-standard XML definition format used to describe ECU
//! calibration layouts (tables, constants, axes, scalings). This parser
//! supports the working subset used by real definition files:
//!
//! - `XDFFORMAT` / `XDFHEADER` (title, author, description, base offset)
//! - `XDFTABLE` with `XDFAXIS` children (`x`, `y` = breakpoints, `z` = data)
//! - `XDFCONSTANT` scalars
//! - `math` scalings as linear expressions in `X` (attribute *or* child-element form)
//! - `embeddeddata` blobs (base64, little-endian, 8/16/32-bit elements)
//!
//! Everything is parsed leniently: unknown elements are skipped, attribute
//! casing is ignored, and files missing optional fields still load.

use crate::ecu::tables::{Scaling, Table2D, ValueWidth};
use anyhow::{bail, Context, Result};

/// A fully parsed XDF document.
#[derive(Debug, Clone, Default)]
pub struct XdfDocument {
    pub title: String,
    pub author: String,
    pub description: String,
    pub base_offset: usize,
    pub tables: Vec<XdfTable>,
    pub constants: Vec<XdfConstant>,
}

/// A table definition from XDF (axes + data location + scaling).
#[derive(Debug, Clone)]
pub struct XdfTable {
    pub id: String,
    pub title: String,
    pub description: String,
    pub units: String,
    pub row_axis: AxisDef,
    pub col_axis: AxisDef,
    pub data: DataDef,
}

/// A constant/scalar definition from XDF.
#[derive(Debug, Clone)]
pub struct XdfConstant {
    pub id: String,
    pub title: String,
    pub units: String,
    pub data: DataDef,
}

/// Axis definition: explicit breakpoints or synthetic count-based ones.
#[derive(Debug, Clone)]
pub struct AxisDef {
    pub units: String,
    pub breakpoints: Option<Vec<f64>>,
    pub count: usize,
    pub scaling: Scaling,
}

/// Raw data location + element format.
#[derive(Debug, Clone)]
pub struct DataDef {
    pub address: usize,
    pub width: ValueWidth,
    pub stride_bits: Option<usize>,
    pub scaling: Scaling,
}

impl XdfDocument {
    /// Parse an XDF file from disk.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read XDF {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("failed to parse XDF {}", path.display()))
    }

    /// Parse XDF text.
    pub fn parse(xml: &str) -> Result<Self> {
        let nodes = parse_lite(xml)?;
        let mut doc = XdfDocument::default();

        // Header fields
        if let Some(h) = find_first(&nodes, "XDFHEADER") {
            doc.title = child_text(h, "deftitle").unwrap_or_default();
            doc.author = child_text(h, "author").unwrap_or_default();
            doc.description = child_text(h, "description").unwrap_or_default();
            if let Some(b) = child_text(h, "baseoffset") {
                doc.base_offset = parse_int_literal(b.trim())? as usize;
            }
        }

        // Tables
        for t in find_all(&nodes, "XDFTABLE") {
            match parse_table(t, doc.base_offset) {
                Ok(tab) => doc.tables.push(tab),
                Err(e) => bail!("XDF table parse error: {:#}", e),
            }
        }

        // Constants
        for c in find_all(&nodes, "XDFCONSTANT") {
            if let Ok(constant) = parse_constant(c, doc.base_offset) {
                doc.constants.push(constant);
            }
        }

        if doc.tables.is_empty() && doc.constants.is_empty() {
            bail!("XDF contains no tables or constants");
        }
        Ok(doc)
    }

    /// Materialize table definitions against an image layout (row/col strides etc.).
    /// Returns ready-to-use [`Table2D`] handles.
    pub fn build_tables(&self) -> Result<Vec<Table2D>> {
        let mut out = Vec::new();
        for t in &self.tables {
            let rows = t
                .row_axis
                .breakpoints
                .clone()
                .unwrap_or_else(|| synthetic_axis(&t.row_axis));
            let cols = t
                .col_axis
                .breakpoints
                .clone()
                .unwrap_or_else(|| synthetic_axis(&t.col_axis));
            let width = t.data.width;
            let col_stride = t
                .data
                .stride_bits
                .map(|b| b / 8)
                .unwrap_or_else(|| width.bytes());
            out.push(Table2D {
                name: t.title.clone(),
                units: t.units.clone(),
                address: t.data.address,
                rows,
                cols,
                width,
                scaling: t.data.scaling,
                col_stride: col_stride.max(width.bytes()),
                row_stride: 0, // computed below once cols known
            });
        }
        // Row stride = full width of one row.
        for t in out.iter_mut() {
            t.row_stride = t.n_cols() * t.col_stride;
        }
        Ok(out)
    }
}

fn synthetic_axis(ax: &AxisDef) -> Vec<f64> {
    (0..ax.count.max(1))
        .map(|i| ax.scaling.to_physical(i as f64))
        .collect()
}

// ---------------------------------------------------------------- XML-lite

#[derive(Debug, Clone)]
pub struct Node {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        let want = name.to_ascii_lowercase();
        self.attrs
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == want)
            .map(|(_, v)| v.as_str())
    }
}

/// Minimal XML scanner: elements, attributes, text. Good enough for XDF.
pub fn parse_lite(xml: &str) -> Result<Vec<Node>> {
    let chars: Vec<char> = xml.chars().collect();
    let mut i = 0usize;
    let mut stack: Vec<Node> = Vec::new();
    let mut roots: Vec<Node> = Vec::new();

    while i < chars.len() {
        if chars[i] == '<' {
            // comment / prolog / doctype: skip to matching '>'
            if chars[i..].starts_with(&['<', '!', '-', '-']) {
                if let Some(end) = find_seq(&chars, i, "-->") {
                    i = end + 3;
                    continue;
                }
            }
            if chars[i..].starts_with(&['<', '?']) || chars[i..].starts_with(&['<', '!']) {
                if let Some(end) = find_char(&chars, i, '>') {
                    i = end + 1;
                    continue;
                }
            }
            // closing tag
            if i + 1 < chars.len() && chars[i + 1] == '/' {
                let end = find_char(&chars, i, '>')
                    .ok_or_else(|| anyhow::anyhow!("unterminated closing tag"))?;
                let tag: String = chars[i + 2..end].iter().filter(|c| !c.is_whitespace()).collect();
                let node = stack
                    .pop()
                    .ok_or_else(|| anyhow::anyhow!("unbalanced closing tag </{}>", tag))?;
                if !node.tag.eq_ignore_ascii_case(&tag) {
                    bail!("mismatched tag: <{}> closed by </{}>", node.tag, tag);
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => roots.push(node),
                }
                i = end + 1;
                continue;
            }
            // opening tag
            let end = find_char(&chars, i, '>')
                .ok_or_else(|| anyhow::anyhow!("unterminated opening tag"))?;
            let inner: String = chars[i + 1..end].iter().collect();
            let self_closing = inner.trim_end().ends_with('/');
            let inner = inner.trim_end().trim_end_matches('/').to_string();
            let (tag, attrs) = split_tag(&inner)?;
            stack.push(Node { tag, attrs, text: String::new(), children: Vec::new() });
            if self_closing {
                let node = stack.pop().unwrap();
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => roots.push(node),
                }
            }
            i = end + 1;
        } else {
            // text content of current node
            let start = i;
            while i < chars.len() && chars[i] != '<' {
                i += 1;
            }
            if let Some(node) = stack.last_mut() {
                node.text.push_str(&chars[start..i].iter().collect::<String>());
            }
        }
    }
    if !stack.is_empty() {
        bail!("unclosed XML element <{}>", stack.last().unwrap().tag);
    }
    Ok(roots)
}

fn find_char(chars: &[char], from: usize, c: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == c)
}

fn find_seq(chars: &[char], from: usize, seq: &str) -> Option<usize> {
    let seq: Vec<char> = seq.chars().collect();
    (from..chars.len().saturating_sub(seq.len() - 1)).find(|&i| chars[i..].starts_with(&seq[..]))
}

fn split_tag(inner: &str) -> Result<(String, Vec<(String, String)>)> {
    let mut chars = inner.chars().peekable();
    let mut tag = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            break;
        }
        tag.push(c);
        chars.next();
    }
    let rest: String = chars.collect();
    let mut attrs = Vec::new();
    let mut it = rest.chars().peekable();
    loop {
        while matches!(it.peek(), Some(c) if c.is_whitespace()) {
            it.next();
        }
        if it.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(&c) = it.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            it.next();
        }
        while matches!(it.peek(), Some(c) if c.is_whitespace()) {
            it.next();
        }
        if it.peek() == Some(&'=') {
            it.next();
            while matches!(it.peek(), Some(c) if c.is_whitespace()) {
                it.next();
            }
            let mut val = String::new();
            match it.peek() {
                Some('"') | Some('\'') => {
                    let quote = it.next().unwrap();
                    for c in it.by_ref() {
                        if c == quote {
                            break;
                        }
                        val.push(c);
                    }
                }
                _ => {
                    while let Some(&c) = it.peek() {
                        if c.is_whitespace() {
                            break;
                        }
                        val.push(c);
                        it.next();
                    }
                }
            }
            attrs.push((key, val));
        } else {
            attrs.push((key, String::new()));
        }
    }
    Ok((tag, attrs))
}

fn find_first<'a>(nodes: &'a [Node], tag: &str) -> Option<&'a Node> {
    for n in nodes {
        if n.tag.eq_ignore_ascii_case(tag) {
            return Some(n);
        }
        if let Some(f) = find_first(&n.children, tag) {
            return Some(f);
        }
    }
    None
}

fn find_all<'a>(nodes: &'a [Node], tag: &str) -> Vec<&'a Node> {
    let mut out = Vec::new();
    for n in nodes {
        if n.tag.eq_ignore_ascii_case(tag) {
            out.push(n);
        }
        out.extend(find_all(&n.children, tag));
    }
    out
}

fn child_text<'a>(node: &'a Node, tag: &str) -> Option<String> {
    node.children
        .iter()
        .find(|c| c.tag.eq_ignore_ascii_case(tag))
        .map(|c| c.text.trim().to_string())
}

// ---------------------------------------------------------------- XDF mapping

fn parse_table(node: &Node, base: usize) -> Result<XdfTable> {
    let id = node.attr("id").unwrap_or("").to_string();
    let title = child_text(node, "title").unwrap_or_else(|| format!("table_{}", id));
    let description = child_text(node, "description").unwrap_or_default();

    let mut row_axis: Option<AxisDef> = None;
    let mut col_axis: Option<AxisDef> = None;
    let mut data: Option<DataDef> = None;
    let mut units = String::new();
    let mut z_units: Option<String> = None;

    for ax in node
        .children
        .iter()
        .filter(|c| c.tag.eq_ignore_ascii_case("XDFAXIS"))
    {
        let axis_id = ax.attr("id").unwrap_or("").to_ascii_lowercase();
        let axis = parse_axis(ax, base)?;
        if units.is_empty() {
            units = axis.units.clone(); // fallback if z has no units
        }
        match axis_id.as_str() {
            "x" => col_axis = Some(axis),
            "y" => row_axis = Some(axis),
            "z" => {
                z_units = Some(axis.units.clone());
                data = Some(data_from_axis(ax, base)?);
            }
            _ => {
                // Some files use type/index instead of id letters.
                if data.is_none() && ax.attr("type").map(|t| t.eq_ignore_ascii_case("z")).unwrap_or(false) {
                    z_units = Some(axis.units.clone());
                    data = Some(data_from_axis(ax, base)?);
                }
            }
        }
    }
    // Table units describe the DATA (z axis), not an axis.
    units = z_units.unwrap_or(units);

    let row_axis = row_axis.unwrap_or_else(|| AxisDef {
        units: String::new(),
        breakpoints: None,
        count: 1,
        scaling: Scaling::IDENTITY,
    });
    let col_axis = col_axis.unwrap_or_else(|| AxisDef {
        units: String::new(),
        breakpoints: None,
        count: 1,
        scaling: Scaling::IDENTITY,
    });
    let data = data.ok_or_else(|| anyhow::anyhow!("table '{}' ({}) has no z/data axis", title, id))?;

    Ok(XdfTable { id, title, description, units, row_axis, col_axis, data })
}

fn parse_constant(node: &Node, base: usize) -> Result<XdfConstant> {
    let id = node.attr("id").unwrap_or("").to_string();
    let title = child_text(node, "title").unwrap_or_else(|| format!("const_{}", id));
    let units = child_text(node, "units").unwrap_or_default();
    let data = data_from_children(node, base)?;
    Ok(XdfConstant { id, title, units, data })
}

fn parse_axis(node: &Node, _base: usize) -> Result<AxisDef> {
    let units = child_text(node, "units").unwrap_or_default();
    let count = child_text(node, "indexcount")
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(1);
    let scaling = scaling_from(node)?;
    let breakpoints = match decode_embedded(node)? {
        Some(raw) => {
            let width = node
                .children
                .iter()
                .find(|c| c.tag.eq_ignore_ascii_case("embeddeddata"))
                .and_then(|d| {
                    d.attr("mmedelementsizebits")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap_or(16);
            let signed = node
                .children
                .iter()
                .find(|c| c.tag.eq_ignore_ascii_case("embeddeddata"))
                .and_then(|d| d.attr("mmedsigned"))
                .map(|s| s.eq_ignore_ascii_case("1") || s.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            let mut vals = Vec::new();
            let elem_bytes = width / 8;
            for chunk in raw.chunks_exact(elem_bytes.max(1)) {
                let rawv = le_int(chunk, signed);
                vals.push(scaling.to_physical(rawv as f64));
            }
            if vals.is_empty() {
                None
            } else {
                Some(vals)
            }
        }
        None => None,
    };
    Ok(AxisDef { units, breakpoints, count, scaling })
}

fn data_from_axis(node: &Node, base: usize) -> Result<DataDef> {
    data_from_children(node, base)
}

fn data_from_children(node: &Node, base: usize) -> Result<DataDef> {
    let scaling = scaling_from(node)?;
    // Find embeddeddata meta (address may also live in <mmedaddress> child).
    let mut address: Option<usize> = None;
    let mut width_bits = 8usize;
    let mut stride_bits: Option<usize> = None;
    let mut signed = false;

    if let Some(d) = node
        .children
        .iter()
        .find(|c| c.tag.eq_ignore_ascii_case("embeddeddata"))
    {
        if let Some(a) = d.attr("mmedaddress").or_else(|| d.attr("address")) {
            address = Some(parse_int_literal(a.trim())? as usize);
        }
        if let Some(b) = d.attr("mmedelementsizebits").or_else(|| d.attr("elementsizebits")) {
            width_bits = b.trim().parse().unwrap_or(8);
        }
        if let Some(b) = d.attr("mmedmajorstridebits").or_else(|| d.attr("majorstridebits")) {
            stride_bits = b.trim().parse().ok();
        }
        if let Some(s) = d.attr("mmedsigned") {
            signed = s.eq_ignore_ascii_case("1") || s.eq_ignore_ascii_case("true");
        }
    }
    // Some definitions put the address on the axis node itself.
    if address.is_none() {
        if let Some(a) = node.attr("mmedaddress").or_else(|| node.attr("address")) {
            address = Some(parse_int_literal(a.trim())? as usize);
        }
    }
    // Or as a child element <mmedaddress>.
    if address.is_none() {
        if let Some(a) = child_text(node, "mmedaddress") {
            address = Some(parse_int_literal(a.trim())? as usize);
        }
    }

    let address = address.ok_or_else(|| {
        anyhow::anyhow!("table data has no mmedaddress (unsupported XDF layout)")
    })?;

    let width = ValueWidth::from_bits(width_bits, signed)?;
    Ok(DataDef {
        address: base + address,
        width,
        stride_bits,
        scaling,
    })
}

/// Pull a linear scaling out of a `math` node — attribute form
/// (`<math equation="X*0.1" tophysical="..." tological="..."/>`) or child form
/// (`<math equation="X"><toPhysical equation="X*0.1"/></math>`).
fn scaling_from(node: &Node) -> Result<Scaling> {
    let math = node
        .children
        .iter()
        .find(|c| c.tag.eq_ignore_ascii_case("math"));

    // Data element math takes priority (z values), then node-level math.
    let data_math = node
        .children
        .iter()
        .find(|c| c.tag.eq_ignore_ascii_case("embeddeddata"))
        .and_then(|d| d.children.iter().find(|c| c.tag.eq_ignore_ascii_case("math")));

    for math in [data_math, math].into_iter().flatten() {
        // toPhysical is the raw→physical direction we want.
        if let Some(tp) = math
            .attr("tophysical")
            .or_else(|| math.attr("toPhysical"))
            .map(|s| s.to_string())
        {
            return Scaling::from_expression(&tp);
        }
        if let Some(child) = math
            .children
            .iter()
            .find(|c| c.tag.eq_ignore_ascii_case("tophysical"))
        {
            let expr = child
                .attr("equation")
                .map(|s| s.to_string())
                .unwrap_or_else(|| child.text.trim().to_string());
            if !expr.is_empty() {
                return Scaling::from_expression(&expr);
            }
        }
        // Fallback: equation alone (some files only carry one form).
        if let Some(eq) = math.attr("equation") {
            if !eq.trim().is_empty() {
                return Scaling::from_expression(eq.trim());
            }
        }
        if !math.text.trim().is_empty() {
            return Scaling::from_expression(math.text.trim());
        }
    }
    Ok(Scaling::IDENTITY)
}

fn decode_embedded(node: &Node) -> Result<Option<Vec<u8>>> {
    for d in node.children.iter().filter(|c| c.tag.eq_ignore_ascii_case("embeddeddata")) {
        let text: String = d.text.chars().filter(|c| !c.is_whitespace()).collect();
        if text.is_empty() {
            continue;
        }
        return Ok(Some(base64_decode(&text)?));
    }
    Ok(None)
}

fn le_int(bytes: &[u8], signed: bool) -> i64 {
    let mut v: u64 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        v |= (b as u64) << (8 * i);
    }
    if signed {
        let bits = bytes.len() * 8;
        ((v << (64 - bits)) as i64) >> (64 - bits)
    } else {
        v as i64
    }
}

/// Parse "0x1A2B" / "4132" style literals.
pub fn parse_int_literal(s: &str) -> Result<i64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).map_err(|_| anyhow::anyhow!("bad hex literal '{}'", s))
    } else {
        s.parse::<i64>().map_err(|_| anyhow::anyhow!("bad integer literal '{}'", s))
    }
}

/// Standard base64 decoder (RFC 4648, tolerates missing padding).
pub fn base64_decode(s: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Result<u8> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => bail!("invalid base64 byte 0x{:02X}", c),
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|&b| b != b'=' && !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc: u32 = 0;
        let mut bits = 0u32;
        for &c in chunk {
            acc = (acc << 6) | val(c)? as u32;
            bits += 6;
        }
        // Whole bytes come off the TOP of the accumulated bit stream;
        // any leftover low bits are base64 padding residue.
        let n_out = (bits / 8) as usize;
        for k in 0..n_out {
            out.push(((acc >> (bits - 8 * (k as u32 + 1))) & 0xFF) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XDF: &str = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
<XDFFORMAT version="1.7">
  <XDFHEADER>
    <description>2002 Camaro LS1 reference layout</description>
    <deftitle>LS1 P01 Demo</deftitle>
    <author>AmericanTune</author>
    <baseoffset>0x0</baseoffset>
    <defaultdigits>1</defaultdigits>
  </XDFHEADER>
  <XDFTABLE id="0x5000">
    <title>VE Table</title>
    <description>Main volumetric efficiency</description>
    <XDFAXIS id="x" type="2" index="0">
      <units>RPM</units>
      <indexcount>4</indexcount>
      <math equation="X*10" />
      <embeddeddata type="2" mmedelementsizebits="16">
        ZADIACwBkAE=   <!-- u16 LE 100,200,300,400 -> X*10 -> 1000..4000 -->
      </embeddeddata>
    </XDFAXIS>
    <XDFAXIS id="y" type="2" index="1">
      <units>kPa</units>
      <indexcount>3</indexcount>
      <math equation="X" />
      <embeddeddata type="2" mmedelementsizebits="16">
        AAAyAGQA   <!-- u16 LE 0,50,100 -->
      </embeddeddata>
    </XDFAXIS>
    <XDFAXIS id="z" type="3">
      <units>g/cyl</units>
      <math equation="X/10" tophysical="X/10" />
      <embeddeddata type="2" mmedaddress="0x5000" mmedelementsizebits="8" mmedmajorstridebits="8">
        VFpgZlheZGpcYmhu   <!-- 12 u8 cells, 3 rows x 4 cols -->
      </embeddeddata>
    </XDFAXIS>
  </XDFTABLE>
  <XDFCONSTANT id="0x6000">
    <title>Rev Limiter</title>
    <units>RPM</units>
    <description>Soft rev limit</description>
    <math equation="X*10" tophysical="X*10" />
    <embeddeddata type="2" mmedaddress="0x6000" mmedelementsizebits="16">
      igI=   <!-- u16 LE 650 -> X*10 -> 6500 rpm -->
    </embeddeddata>
  </XDFCONSTANT>
</XDFFORMAT>"#;

    #[test]
    fn parses_header_tables_constants() {
        let doc = XdfDocument::parse(SAMPLE_XDF).unwrap();
        assert_eq!(doc.title, "LS1 P01 Demo");
        assert_eq!(doc.author, "AmericanTune");
        assert_eq!(doc.tables.len(), 1);
        assert_eq!(doc.constants.len(), 1);

        let t = &doc.tables[0];
        assert_eq!(t.title, "VE Table");
        assert_eq!(t.data.address, 0x5000);
        // z scaling from "X/10"
        assert!((t.data.scaling.factor - 0.1).abs() < 1e-12);
    }

    #[test]
    fn axis_breakpoints_are_decoded() {
        let doc = XdfDocument::parse(SAMPLE_XDF).unwrap();
        let t = &doc.tables[0];
        let cols = t.col_axis.breakpoints.as_ref().unwrap();
        // raw u16 100,200,300,400 with scaling X*10 -> 1000..4000 RPM
        assert_eq!(cols, &vec![1000.0, 2000.0, 3000.0, 4000.0]);
        let rows = t.row_axis.breakpoints.as_ref().unwrap();
        // raw u16 0,50,100 with identity scaling
        assert_eq!(rows, &vec![0.0, 50.0, 100.0]);
    }

    #[test]
    fn build_tables_produces_usable_grid() {
        let doc = XdfDocument::parse(SAMPLE_XDF).unwrap();
        let tables = doc.build_tables().unwrap();
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.n_cols(), 4);
        assert_eq!(t.n_rows(), 3);
    }

    #[test]
    fn constant_parses_with_scaling() {
        let doc = XdfDocument::parse(SAMPLE_XDF).unwrap();
        let c = &doc.constants[0];
        assert_eq!(c.title, "Rev Limiter");
        assert_eq!(c.data.address, 0x6000);
        assert!((c.data.scaling.factor - 10.0).abs() < 1e-12);
    }

    #[test]
    fn base64_vectors() {
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
        assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(base64_decode("TQ").unwrap(), b"M");
        assert!(base64_decode("****").is_err());
    }

    #[test]
    fn child_form_math_is_supported() {
        let xdf = r#"<XDFFORMAT version="1.7"><XDFHEADER><deftitle>t</deftitle></XDFHEADER>
        <XDFTABLE id="0x10"><title>T</title>
          <XDFAXIS id="x"><indexcount>1</indexcount></XDFAXIS>
          <XDFAXIS id="y"><indexcount>1</indexcount></XDFAXIS>
          <XDFAXIS id="z">
            <math equation="X"><toPhysical equation="X*2"/></math>
            <embeddeddata mmedaddress="0x10" mmedelementsizebits="8">AQ==</embeddeddata>
          </XDFAXIS>
        </XDFTABLE></XDFFORMAT>"#;
        let doc = XdfDocument::parse(xdf).unwrap();
        assert!((doc.tables[0].data.scaling.factor - 2.0).abs() < 1e-12);
    }

    #[test]
    fn malformed_xml_is_rejected() {
        assert!(XdfDocument::parse("<XDFFORMAT><oops>").is_err());
        assert!(XdfDocument::parse("<XDFFORMAT></wrong>").is_err());
    }
}

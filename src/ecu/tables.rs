//! Table engine: scaled integer storage ⇄ physical units, editing, interpolation, smoothing.
//!
//! Real ECUs store calibration tables as u8/u16/i16 scaled integers (not f32),
//! with a linear mapping `physical = raw * factor + offset` (or arbitrary linear
//! expressions extracted from XDF `math` equations).

use anyhow::{bail, Result};

/// Integer width of one table cell in the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueWidth {
    U8,
    I8,
    U16,
    I16,
    U32,
}

impl ValueWidth {
    pub fn bytes(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 => 4,
        }
    }

    pub fn from_bits(bits: usize, signed: bool) -> Result<Self> {
        Ok(match (bits, signed) {
            (8, false) => Self::U8,
            (8, true) => Self::I8,
            (16, false) => Self::U16,
            (16, true) => Self::I16,
            (32, _) => Self::U32,
            _ => bail!("unsupported element size {} bits (signed={})", bits, signed),
        })
    }
}

/// Linear scaling: `physical = raw * factor + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scaling {
    pub factor: f64,
    pub offset: f64,
}

impl Scaling {
    pub const IDENTITY: Scaling = Scaling { factor: 1.0, offset: 0.0 };

    /// Extract a linear scaling from a math expression in `X`
    /// (e.g. "X*0.1", "X/10-40", "(X-100)*2.5") by probing f(0), f(1), f(2).
    /// Linear maps satisfy f(x) = factor*x + offset exactly — the third probe
    /// rejects non-linear expressions (e.g. "X*X") that coincidentally match at 0/1.
    pub fn from_expression(expr: &str) -> Result<Self> {
        let f0 = eval_linear(expr, 0.0)?;
        let f1 = eval_linear(expr, 1.0)?;
        let f2 = eval_linear(expr, 2.0)?;
        let factor = f1 - f0;
        if !factor.is_finite() || !f0.is_finite() {
            bail!("scaling expression '{}' is not finite", expr);
        }
        let predicted = factor * 2.0 + f0;
        if (f2 - predicted).abs() > 1e-9 * predicted.abs().max(1.0) {
            bail!("scaling expression '{}' is not linear in X", expr);
        }
        Ok(Self { factor, offset: f0 })
    }

    pub fn to_physical(&self, raw: f64) -> f64 {
        raw * self.factor + self.offset
    }

    pub fn to_raw(&self, physical: f64) -> f64 {
        (physical - self.offset) / self.factor
    }
}

/// Tiny recursive-descent evaluator for linear math expressions in `X`.
/// Supports + - * / parentheses, unary minus, numeric literals, and `X`/`x`.
/// (Multiplicative nesting of X is rejected — that would be non-linear.)
pub fn eval_linear(expr: &str, x: f64) -> Result<f64> {
    let tokens = tokenize(expr)?;
    let mut pos = 0usize;
    let v = parse_expr(&tokens, &mut pos, x)?;
    if pos != tokens.len() {
        bail!("unexpected trailing tokens in expression '{}'", expr);
    }
    Ok(v)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    X,
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Result<Vec<Tok>> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => { out.push(Tok::Plus); i += 1; }
            '-' => { out.push(Tok::Minus); i += 1; }
            '*' => { out.push(Tok::Star); i += 1; }
            '/' => { out.push(Tok::Slash); i += 1; }
            '(' => { out.push(Tok::LParen); i += 1; }
            ')' => { out.push(Tok::RParen); i += 1; }
            'X' | 'x' => { out.push(Tok::X); i += 1; }
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let lit: String = chars[start..i].iter().collect();
                out.push(Tok::Num(lit.parse::<f64>().map_err(|_| {
                    anyhow::anyhow!("bad number literal '{}' in expression", lit)
                })?));
            }
            other => bail!("unexpected character '{}' in scaling expression", other),
        }
    }
    Ok(out)
}

fn parse_expr(t: &[Tok], pos: &mut usize, x: f64) -> Result<f64> {
    let mut v = parse_term(t, pos, x)?;
    while *pos < t.len() {
        match &t[*pos] {
            Tok::Plus => { *pos += 1; v += parse_term(t, pos, x)?; }
            Tok::Minus => { *pos += 1; v -= parse_term(t, pos, x)?; }
            _ => break,
        }
    }
    Ok(v)
}

fn parse_term(t: &[Tok], pos: &mut usize, x: f64) -> Result<f64> {
    let mut v = parse_factor(t, pos, x)?;
    while *pos < t.len() {
        match &t[*pos] {
            Tok::Star => { *pos += 1; v *= parse_factor(t, pos, x)?; }
            Tok::Slash => {
                *pos += 1;
                let d = parse_factor(t, pos, x)?;
                if d == 0.0 {
                    bail!("division by zero in scaling expression");
                }
                v /= d;
            }
            _ => break,
        }
    }
    Ok(v)
}

fn parse_factor(t: &[Tok], pos: &mut usize, x: f64) -> Result<f64> {
    if *pos >= t.len() {
        bail!("unexpected end of scaling expression");
    }
    match &t[*pos] {
        Tok::Minus => {
            *pos += 1;
            Ok(-parse_factor(t, pos, x)?)
        }
        Tok::Plus => {
            *pos += 1;
            parse_factor(t, pos, x)
        }
        Tok::Num(n) => {
            *pos += 1;
            Ok(*n)
        }
        Tok::X => {
            *pos += 1;
            Ok(x)
        }
        Tok::LParen => {
            *pos += 1;
            let v = parse_expr(t, pos, x)?;
            if *pos >= t.len() || t[*pos] != Tok::RParen {
                bail!("missing closing parenthesis");
            }
            *pos += 1;
            Ok(v)
        }
        other => bail!("unexpected token {:?} in scaling expression", other),
    }
}

/// A 2-D calibration table backed by an ECU image.
#[derive(Debug, Clone)]
pub struct Table2D {
    pub name: String,
    pub units: String,
    pub address: usize,
    /// Row axis (e.g. MAP / load) — physical values, ascending.
    pub rows: Vec<f64>,
    /// Column axis (e.g. RPM) — physical values, ascending.
    pub cols: Vec<f64>,
    pub width: ValueWidth,
    pub scaling: Scaling,
    /// Major stride in bytes between consecutive columns (default = width).
    pub col_stride: usize,
    /// Stride in bytes between consecutive rows.
    pub row_stride: usize,
}

impl Table2D {
    pub fn n_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn n_cols(&self) -> usize {
        self.cols.len()
    }

    fn cell_addr(&self, r: usize, c: usize) -> usize {
        self.address + r * self.row_stride + c * self.col_stride
    }

    /// Read one cell as a physical value.
    pub fn get(&self, img: &EcuImageRef<'_>, r: usize, c: usize) -> Result<f64> {
        if r >= self.n_rows() || c >= self.n_cols() {
            bail!("cell ({},{}) outside {}x{} table '{}'", r, c, self.n_rows(), self.n_cols(), self.name);
        }
        let addr = self.cell_addr(r, c);
        let raw = match self.width {
            ValueWidth::U8 => img.read_uint(addr, 1)? as f64,
            ValueWidth::I8 => img.read_int(addr, 1)? as f64,
            ValueWidth::U16 => img.read_uint(addr, 2)? as f64,
            ValueWidth::I16 => img.read_int(addr, 2)? as f64,
            ValueWidth::U32 => img.read_uint(addr, 4)? as f64,
        };
        Ok(self.scaling.to_physical(raw))
    }

    /// Write one cell from a physical value (raw value rounded to nearest int).
    pub fn set(&self, img: &mut EcuImageMut<'_>, r: usize, c: usize, physical: f64) -> Result<f64> {
        if r >= self.n_rows() || c >= self.n_cols() {
            bail!("cell ({},{}) outside {}x{} table '{}'", r, c, self.n_rows(), self.n_cols(), self.name);
        }
        let raw_f = self.scaling.to_raw(physical);
        let raw_f = raw_f.round();
        let addr = self.cell_addr(r, c);
        match self.width {
            ValueWidth::U8 => {
                let v = clamp_int(raw_f, 0.0, 255.0) as u64;
                img.write_uint(addr, 1, v)?;
            }
            ValueWidth::I8 => {
                let v = clamp_int(raw_f, -128.0, 127.0) as i64 as u64;
                img.write_uint(addr, 1, v & 0xFF)?;
            }
            ValueWidth::U16 => {
                let v = clamp_int(raw_f, 0.0, 65535.0) as u64;
                img.write_uint(addr, 2, v)?;
            }
            ValueWidth::I16 => {
                let v = clamp_int(raw_f, -32768.0, 32767.0) as i64 as u64;
                img.write_uint(addr, 2, v & 0xFFFF)?;
            }
            ValueWidth::U32 => {
                let v = clamp_int(raw_f, 0.0, 4294967295.0) as u64;
                img.write_uint(addr, 4, v)?;
            }
        }
        // Return the value actually stored (after rounding + clamping).
        self.get(&EcuImageRef(img.0), r, c)
    }

    /// Read the whole grid as physical values: grid[r][c].
    pub fn grid(&self, img: &EcuImageRef<'_>) -> Result<Vec<Vec<f64>>> {
        (0..self.n_rows())
            .map(|r| (0..self.n_cols()).map(|c| self.get(img, r, c)).collect())
            .collect()
    }

    /// Bilinear interpolation at an operating point (col axis, row axis).
    pub fn interpolate(&self, img: &EcuImageRef<'_>, col_x: f64, row_y: f64) -> Result<f64> {
        let (c0, c1, cf) = bracket(&self.cols, col_x);
        let (r0, r1, rf) = bracket(&self.rows, row_y);
        let v00 = self.get(img, r0, c0)?;
        let v01 = self.get(img, r0, c1)?;
        let v10 = self.get(img, r1, c0)?;
        let v11 = self.get(img, r1, c1)?;
        let top = v00 + (v01 - v00) * cf;
        let bot = v10 + (v11 - v10) * cf;
        Ok(top + (bot - top) * rf)
    }

    /// Apply a 3x3 smoothing kernel to interior cells; returns number of cells changed.
    pub fn smooth(&self, img: &mut EcuImageMut<'_>) -> Result<usize> {
        let grid = self.grid(&EcuImageRef(img.0))?;
        let mut changed = 0usize;
        for r in 1..self.n_rows().saturating_sub(1) {
            for c in 1..self.n_cols().saturating_sub(1) {
                let mut sum = 0.0;
                for dr in -1i32..=1 {
                    for dc in -1i32..=1 {
                        if dr == 0 && dc == 0 {
                            continue;
                        }
                        sum += grid[(r as i32 + dr) as usize][(c as i32 + dc) as usize];
                    }
                }
                let avg = sum / 8.0;
                if (grid[r][c] - avg).abs() > 1e-9 {
                    self.set(img, r, c, avg)?;
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

    pub fn stats(&self, img: &EcuImageRef<'_>) -> Result<TableStats> {
        let grid = self.grid(img)?;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut sum = 0.0;
        let mut n = 0usize;
        for row in &grid {
            for &v in row {
                min = min.min(v);
                max = max.max(v);
                sum += v;
                n += 1;
            }
        }
        Ok(TableStats { min, max, mean: sum / n.max(1) as f64, cells: n })
    }

    /// Total byte footprint of this table in the image.
    pub fn byte_len(&self) -> usize {
        (self.n_rows() - 1) * self.row_stride + (self.n_cols() - 1) * self.col_stride + self.width.bytes()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TableStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub cells: usize,
}

fn clamp_int(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// Find the two adjacent breakpoints surrounding `x` and the interpolation fraction.
fn bracket(axis: &[f64], x: f64) -> (usize, usize, f64) {
    let n = axis.len();
    if n == 1 || x <= axis[0] {
        return (0, 0, 0.0);
    }
    if x >= axis[n - 1] {
        return (n - 1, n - 1, 0.0);
    }
    for i in 0..n - 1 {
        if x <= axis[i + 1] {
            let span = axis[i + 1] - axis[i];
            let f = if span.abs() < f64::EPSILON { 0.0 } else { (x - axis[i]) / span };
            return (i, i + 1, f);
        }
    }
    (n - 1, n - 1, 0.0)
}

// ---- thin borrow wrappers so tables can work with either EcuImage mutability ----
use crate::ecu::bin::EcuImage;
pub struct EcuImageRef<'a>(pub &'a EcuImage);
pub struct EcuImageMut<'a>(pub &'a mut EcuImage);

impl EcuImageRef<'_> {
    pub fn read_uint(&self, addr: usize, width: usize) -> Result<u64> {
        self.0.read_uint(addr, width)
    }
    pub fn read_int(&self, addr: usize, width: usize) -> Result<i64> {
        self.0.read_int(addr, width)
    }
}

impl EcuImageMut<'_> {
    pub fn write_uint(&mut self, addr: usize, width: usize, value: u64) -> Result<()> {
        self.0.write_uint(addr, width, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_parses_common_linear_forms() {
        let s = Scaling::from_expression("X*0.1").unwrap();
        assert!((s.to_physical(100.0) - 10.0).abs() < 1e-9);

        let s = Scaling::from_expression("X/10-40").unwrap();
        assert!((s.to_physical(2931.0) - 253.1).abs() < 1e-6);
        assert!((s.to_raw(253.1) - 2931.0).abs() < 1e-6);

        let s = Scaling::from_expression("(X-100)*2.5").unwrap();
        assert!((s.to_physical(110.0) - 25.0).abs() < 1e-9);
    }

    #[test]
    fn scaling_rejects_nonsense() {
        assert!(Scaling::from_expression("X*X").is_err());
        assert!(Scaling::from_expression("hello").is_err());
    }

    fn make_img() -> EcuImage {
        EcuImage::new(vec![0u8; 256], "test").unwrap()
    }

    fn make_table() -> Table2D {
        Table2D {
            name: "VE".into(),
            units: "g/cyl".into(),
            address: 16,
            rows: vec![0.0, 50.0, 100.0],
            cols: vec![1000.0, 2000.0, 3000.0, 4000.0],
            width: ValueWidth::U8,
            scaling: Scaling { factor: 0.1, offset: 0.0 },
            col_stride: 1,
            row_stride: 4,
        }
    }

    #[test]
    fn cell_roundtrip_respects_scaling() {
        let mut img = make_img();
        let t = make_table();
        {
            let mut m = EcuImageMut(&mut img);
            let stored = t.set(&mut m, 1, 2, 8.4).unwrap();
            assert!((stored - 8.4).abs() < 1e-9);
        }
        let v = t.get(&EcuImageRef(&img), 1, 2).unwrap();
        assert!((v - 8.4).abs() < 1e-9);
    }

    #[test]
    fn raw_storage_is_u8_scaled() {
        let mut img = make_img();
        let t = make_table();
        t.set(&mut EcuImageMut(&mut img), 0, 0, 10.0).unwrap();
        // raw = 10.0 / 0.1 = 100
        assert_eq!(img.data()[16], 100);
    }

    #[test]
    fn interpolation_matches_corners_and_center() {
        let mut img = make_img();
        let t = make_table();
        {
            let mut m = EcuImageMut(&mut img);
            for r in 0..3 {
                for c in 0..4 {
                    t.set(&mut m, r, c, (r * 10 + c) as f64).unwrap();
                }
            }
        }
        let e = EcuImageRef(&img);
        assert!((t.interpolate(&e, 1000.0, 0.0).unwrap() - 0.0).abs() < 1e-9);
        assert!((t.interpolate(&e, 2000.0, 50.0).unwrap() - 11.0).abs() < 1e-9);
        assert!((t.interpolate(&e, 4000.0, 100.0).unwrap() - 23.0).abs() < 1e-9);
    }

    #[test]
    fn smooth_reduces_a_spike() {
        let mut img = make_img();
        let t = make_table();
        {
            let mut m = EcuImageMut(&mut img);
            for r in 0..3 {
                for c in 0..4 {
                    t.set(&mut m, r, c, 10.0).unwrap();
                }
            }
            t.set(&mut m, 1, 1, 90.0).unwrap(); // spike
        }
        let changed = t.smooth(&mut EcuImageMut(&mut img)).unwrap();
        assert!(changed > 0);
        let v = t.get(&EcuImageRef(&img), 1, 1).unwrap();
        assert!((v - 10.0).abs() < 0.5, "spike should be pulled toward neighbors, got {}", v);
    }

    #[test]
    fn stats_are_correct() {
        let mut img = make_img();
        let t = make_table();
        {
            let mut m = EcuImageMut(&mut img);
            t.set(&mut m, 0, 0, 5.0).unwrap();
            t.set(&mut m, 0, 1, 15.0).unwrap();
        }
        let s = t.stats(&EcuImageRef(&img)).unwrap();
        assert_eq!(s.cells, 12);
        assert!((s.min - 0.0).abs() < 1e-9);
        assert!((s.max - 15.0).abs() < 1e-9);
    }
}

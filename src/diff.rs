//! Image + table-aware diffing: see exactly which calibration cells changed.

use crate::ecu::bin::EcuImage;
use crate::ecu::tables::{EcuImageRef, Table2D};
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct ByteDiff {
    pub offset: usize,
    pub a: u8,
    pub b: u8,
}

#[derive(Debug, Clone)]
pub struct TableDiff {
    pub table: String,
    pub cell: (usize, usize),
    pub a: f64,
    pub b: f64,
}

#[derive(Debug, Clone)]
pub struct DiffReport {
    pub bytes: Vec<ByteDiff>,
    pub tables: Vec<TableDiff>,
    pub total_bytes: usize,
}

impl DiffReport {
    pub fn is_identical(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Raw byte diff over the overlapping region.
pub fn diff_images(a: &EcuImage, b: &EcuImage) -> Vec<ByteDiff> {
    let n = a.len().min(b.len());
    let mut out = Vec::new();
    for i in 0..n {
        if a.data()[i] != b.data()[i] {
            out.push(ByteDiff { offset: i, a: a.data()[i], b: b.data()[i] });
        }
    }
    out
}

/// Full report: byte diffs plus cell-level attribution via table definitions.
pub fn diff_with_tables(a: &EcuImage, b: &EcuImage, tables: &[Table2D]) -> Result<DiffReport> {
    let bytes = diff_images(a, b);
    let ea = EcuImageRef(a);
    let eb = EcuImageRef(b);
    let mut tdiffs = Vec::new();
    for t in tables {
        for r in 0..t.n_rows() {
            for c in 0..t.n_cols() {
                let va = t.get(&ea, r, c)?;
                let vb = t.get(&eb, r, c)?;
                if (va - vb).abs() > 1e-9 {
                    tdiffs.push(TableDiff { table: t.name.clone(), cell: (r, c), a: va, b: vb });
                }
            }
        }
    }
    Ok(DiffReport { total_bytes: bytes.len(), bytes, tables: tdiffs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecu::tables::{EcuImageMut, Scaling, ValueWidth};

    fn table() -> Table2D {
        Table2D {
            name: "VE".into(),
            units: "g/cyl".into(),
            address: 8,
            rows: vec![50.0, 100.0],
            cols: vec![1000.0, 2000.0],
            width: ValueWidth::U8,
            scaling: Scaling { factor: 0.1, offset: 0.0 },
            col_stride: 1,
            row_stride: 2,
        }
    }

    #[test]
    fn attributes_changes_to_cells() {
        let mut a = EcuImage::new(vec![0u8; 64], "a").unwrap();
        let mut b = a.clone();
        let t = table();
        t.set(&mut EcuImageMut(&mut a), 0, 0, 5.0).unwrap();
        t.set(&mut EcuImageMut(&mut b), 0, 0, 8.0).unwrap();
        t.set(&mut EcuImageMut(&mut b), 1, 1, 9.0).unwrap();

        let rep = diff_with_tables(&a, &b, &[t]).unwrap();
        assert_eq!(rep.total_bytes, 2);
        assert_eq!(rep.tables.len(), 2);
        assert!(rep.tables.iter().any(|d| d.cell == (0, 0) && (d.a - 5.0).abs() < 1e-9));
    }

    #[test]
    fn identical_images_diff_clean() {
        let a = EcuImage::new(vec![1u8; 32], "a").unwrap();
        let b = a.clone();
        let rep = diff_with_tables(&a, &b, &[]).unwrap();
        assert!(rep.is_identical());
    }
}

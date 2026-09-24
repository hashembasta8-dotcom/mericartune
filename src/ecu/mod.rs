//! ECU Binary parsing and XDF definition support
//! 
//! Supports GM P01/P59 (LS1/LS6, LS2/LS3/LS7), Ford, Dodge ECUs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use anyhow::Result;

/// Represents a complete ECU definition loaded from XDF
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcuDefinition {
    pub ecu_id: String,
    pub family: EcuFamily,
    pub name: String,
    pub version: String,
    pub memory_segments: Vec<MemorySegment>,
    pub tables: Vec<TableDefinition>,
    pub scalars: Vec<ScalarDefinition>,
    pub supported_os_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EcuFamily {
    /// GM P01 - LS1/LS6 based (1997-2004)
    GmP01Ls1Ls6,
    /// GM P59 - LS2/LS3/LS7 based (2005-2007)
    GmP59Ls2Ls3Ls7,
    /// GM LT-based (Gen 5/6, 2015+)
    GmLt,
    /// Ford Coyote 5.0/5.2
    FordCoyote,
    /// Ford Power Stroke Diesel
    FordPowerStroke,
    /// Ford Ecoboost
    FordEcoboost,
    /// Dodge Hemi
    DodgeHemi,
    /// Dodge Cummins Diesel
    DodgeCummins,
    /// Generic/Unknown
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySegment {
    pub name: String,
    pub start_address: u32,
    pub size: usize,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDefinition {
    pub name: String,
    pub address: u32,
    pub size: usize,
    pub x_axis: Vec<f32>,  // X-axis values (e.g., RPM)
    pub y_axis: Vec<f32>,  // Y-axis values (e.g., Load)
    pub z_values: Vec<Vec<f32>>,  // 2D table data
    pub unit: String,
    pub min_value: f32,
    pub max_value: f32,
    pub default_value: f32,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalarDefinition {
    pub name: String,
    pub address: u32,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub unit: String,
    pub scale: f32,
    pub offset: f32,
    pub min_value: f32,
    pub max_value: f32,
    pub default_value: f32,
    pub description: String,
}

/// Parsed ECU binary ready for editing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcuBinary {
    pub data: Vec<u8>,
    pub definition: EcuDefinition,
    pub os_id: String,
    pub checksum: Option<String>,
    pub metadata: EcuMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcuMetadata {
    pub hardware_id: String,
    pub calibration_id: String,
    pub vehicle_make: String,
    pub vehicle_model: String,
    pub vin: Option<String>,
    pub flash_date: Option<String>,
}

/// Parser for ECU binary files
pub struct EcuParser {
    definitions: HashMap<String, EcuDefinition>,
}

impl EcuParser {
    /// Create a new ECU parser with loaded definitions
    pub fn new() -> Self {
        Self {
            definitions: HashMap::new(),
        }
    }
    
    /// Load an XDF definition file
    pub fn load_definition(&mut self, path: &PathBuf) -> Result<&EcuDefinition> {
        let content = std::fs::read_to_string(path)?;
        let def: EcuDefinition = serde_json::from_str(&content)?;
        let id = def.ecu_id.clone();
        self.definitions.insert(id.clone(), def);
        Ok(self.definitions.get(&id).unwrap())
    }
    
    /// Parse an ECU binary using a known definition
    pub fn parse(&self, data: Vec<u8>, def_id: &str) -> Result<EcuBinary> {
        let def = self.definitions.get(def_id)
            .ok_or_else(|| anyhow::anyhow!("Definition {} not found", def_id))?;
        
        let os_id = self.extract_os_id(&data)?;
        
        Ok(EcuBinary {
            data,
            definition: def.clone(),
            os_id,
            checksum: self.calculate_checksum(&data),
            metadata: EcuMetadata {
                hardware_id: String::new(),
                calibration_id: String::new(),
                vehicle_make: String::new(),
                vehicle_model: String::new(),
                vin: None,
                flash_date: None,
            },
        })
    }
    
    /// Extract OS/Operating System ID from binary
    fn extract_os_id(&self, data: &[u8]) -> Result<String> {
        // GM P01/P59 store OS ID at specific offsets
        // Standard offset varies by PCM type
        if data.len() > 1284 {
            let bytes = &data[1280..1288];
            return Ok(hex::encode(bytes));
        }
        Ok(String::from("unknown"))
    }
    
    /// Calculate checksum for validation
    fn calculate_checksum(&self, data: &[u8]) -> Option<String> {
        if data.is_empty() {
            return None;
        }
        let sum: u32 = data.iter().map(|&b| b as u32).sum();
        Some(format!("{:08X}", sum & 0xFFFFFFFF))
    }
    
    /// Edit a table value in the binary
    pub fn edit_table(&mut self, binary: &mut EcuBinary, table_name: &str, 
                      x_idx: usize, y_idx: usize, value: f32) -> Result<()> {
        let def = &binary.definition;
        let table = def.tables.iter()
            .find(|t| t.name == table_name)
            .ok_or_else(|| anyhow::anyhow!("Table {} not found", table_name))?;
        
        // Calculate byte offset for the cell
        let cell_index = y_idx * table.x_axis.len() + x_idx;
        let byte_offset = table.address as usize + (cell_index * 4); // Assuming f32
        
        if byte_offset + 4 > binary.data.len() {
            return anyhow::bail!("Table edit out of bounds");
        }
        
        let bytes = value.to_le_bytes();
        binary.data[byte_offset..byte_offset + 4].copy_from_slice(&bytes);
        
        Ok(())
    }
    
    /// Edit a scalar value
    pub fn edit_scalar(&mut self, binary: &mut EcuBinary, scalar_name: &str, 
                       value: f32) -> Result<()> {
        let def = &binary.definition;
        let scalar = def.scalars.iter()
            .find(|s| s.name == scalar_name)
            .ok_or_else(|| anyhow::anyhow!("Scalar {} not found", scalar_name))?;
        
        let offset = scalar.address as usize + scalar.byte_offset;
        let raw_value = ((value - scalar.offset) / scalar.scale) as u32;
        let bytes = raw_value.to_le_bytes();
        
        if offset + scalar.byte_length > binary.data.len() {
            return anyhow::bail!("Scalar edit out of bounds");
        }
        
        binary.data[offset..offset + scalar.byte_length].copy_from_slice(&bytes);
        
        Ok(())
    }
    
    /// Get all available tables for display
    pub fn get_tables(&self, binary: &EcuBinary) -> Vec<&TableDefinition> {
        binary.definition.tables.iter().collect()
    }
    
    /// Get all available scalars
    pub fn get_scalars(&self, binary: &EcuBinary) -> Vec<&ScalarDefinition> {
        binary.definition.scalars.iter().collect()
    }
}

/// Factory for creating common ECU definitions
pub mod definitions {
    use super::*;
    
    /// Create GM P01 (LS1/LS6) definition
    pub fn gm_p01_ls1() -> EcuDefinition {
        EcuDefinition {
            ecu_id: "gm_p01_512k".to_string(),
            family: EcuFamily::GmP01Ls1Ls6,
            name: "GM P01 512K LS1/LS6".to_string(),
            version: "1.0".to_string(),
            memory_segments: vec![
                MemorySegment {
                    name: "Operating System".to_string(),
                    start_address: 0x000000,
                    size: 0x20000,
                    description: "GM operating system code".to_string(),
                },
                MemorySegment {
                    name: "Calibration Area".to_string(),
                    start_address: 0x20000,
                    size: 0x40000,
                    description: "Tuneable calibration tables".to_string(),
                },
                MemorySegment {
                    name: "Checksum".to_string(),
                    start_address: 0x60000,
                    size: 0x1000,
                    description: "Validation checksums".to_string(),
                },
            ],
            tables: vec![
                TableDefinition {
                    name: "Fuel_LT1".to_string(),
                    address: 0x20000,
                    size: 192, // 16x12 table
                    x_axis: vec![1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 3500.0, 4000.0, 4500.0, 5000.0, 5500.0, 6000.0],
                    y_axis: vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0],
                    z_values: vec![vec![0.0; 11]; 6],
                    unit: "ms".to_string(),
                    min_value: 0.0,
                    max_value: 30.0,
                    default_value: 8.0,
                    description: "Fuel LT1 - Injector pulse width".to_string(),
                },
                TableDefinition {
                    name: "Spark_LT1".to_string(),
                    address: 0x24000,
                    size: 192,
                    x_axis: vec![1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 3500.0, 4000.0, 4500.0, 5000.0, 5500.0, 6000.0],
                    y_axis: vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0],
                    z_values: vec![vec![0.0; 11]; 6],
                    unit: "deg".to_string(),
                    min_value: 0.0,
                    max_value: 50.0,
                    default_value: 20.0,
                    description: "Spark timing LT1".to_string(),
                },
            ],
            scalars: vec![
                ScalarDefinition {
                    name: "Rev_Limiter".to_string(),
                    address: 0x28000,
                    byte_offset: 0,
                    byte_length: 2,
                    unit: "rpm".to_string(),
                    scale: 1.0,
                    offset: 0.0,
                    min_value: 4000.0,
                    max_value: 8000.0,
                    default_value: 6500.0,
                    description: "Engine rev limiter".to_string(),
                },
                ScalarDefinition {
                    name: "Speed_Limiter".to_string(),
                    address: 0x28002,
                    byte_offset: 0,
                    byte_length: 2,
                    unit: "mph".to_string(),
                    scale: 1.0,
                    offset: 0.0,
                    min_value: 80.0,
                    max_value: 220.0,
                    default_value: 155.0,
                    description: "Vehicle speed limiter".to_string(),
                },
            ],
            supported_os_ids: vec![
                "12208322".to_string(),
                "12208332".to_string(),
                "12208342".to_string(),
            ],
        }
    }
    
    /// Create GM P59 (LS2/LS3/LS7) definition
    pub fn gm_p59_ls2() -> EcuDefinition {
        EcuDefinition {
            ecu_id: "gm_p59_1m_ls2".to_string(),
            family: EcuFamily::GmP59Ls2Ls3Ls7,
            name: "GM P59 1M LS2/LS3/LS7".to_string(),
            version: "1.0".to_string(),
            memory_segments: vec![
                MemorySegment {
                    name: "Operating System".to_string(),
                    start_address: 0x000000,
                    size: 0x20000,
                    description: "GM operating system code".to_string(),
                },
                MemorySegment {
                    name: "Calibration Area".to_string(),
                    start_address: 0x20000,
                    size: 0x80000,
                    description: "Tuneable calibration tables".to_string(),
                },
            ],
            tables: vec![
                TableDefinition {
                    name: "Fuel_LT1".to_string(),
                    address: 0x20000,
                    size: 256,
                    x_axis: vec![1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 3500.0, 4000.0, 4500.0, 5000.0, 5500.0, 6000.0, 6500.0],
                    y_axis: vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0],
                    z_values: vec![vec![0.0; 12]; 6],
                    unit: "ms".to_string(),
                    min_value: 0.0,
                    max_value: 30.0,
                    default_value: 8.0,
                    description: "Fuel LT1 - P59 variant".to_string(),
                },
                TableDefinition {
                    name: "Spark_LT1".to_string(),
                    address: 0x28000,
                    size: 256,
                    x_axis: vec![1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 3500.0, 4000.0, 4500.0, 5000.0, 5500.0, 6000.0, 6500.0],
                    y_axis: vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0],
                    z_values: vec![vec![0.0; 12]; 6],
                    unit: "deg".to_string(),
                    min_value: 0.0,
                    max_value: 50.0,
                    default_value: 22.0,
                    description: "Spark timing LT1 - P59 variant".to_string(),
                },
            ],
            scalars: vec![
                ScalarDefinition {
                    name: "Rev_Limiter".to_string(),
                    address: 0x38000,
                    byte_offset: 0,
                    byte_length: 2,
                    unit: "rpm".to_string(),
                    scale: 1.0,
                    offset: 0.0,
                    min_value: 4000.0,
                    max_value: 8000.0,
                    default_value: 6800.0,
                    description: "Engine rev limiter P59".to_string(),
                },
            ],
            supported_os_ids: vec![
                "12587811".to_string(),
                "12605114".to_string(),
                "12606807".to_string(),
                "12613246".to_string(),
                "12629623".to_string(),
            ],
        }
    }
    
    /// Create Ford Coyote definition
    pub fn ford_coyote_50() -> EcuDefinition {
        EcuDefinition {
            ecu_id: "ford_coyote_50".to_string(),
            family: EcuFamily::FordCoyote,
            name: "Ford Coyote 5.0L".to_string(),
            version: "1.0".to_string(),
            memory_segments: vec![
                MemorySegment {
                    name: "Calibration".to_string(),
                    start_address: 0x000000,
                    size: 0x40000,
                    description: "Ford Coyote calibration area".to_string(),
                },
            ],
            tables: vec![
                TableDefinition {
                    name: "Fuel".to_string(),
                    address: 0x10000,
                    size: 128,
                    x_axis: vec![1000.0; 8],
                    y_axis: vec![0.0; 4],
                    z_values: vec![vec![0.0; 8]; 4],
                    unit: "ms".to_string(),
                    min_value: 0.0,
                    max_value: 30.0,
                    default_value: 7.5,
                    description: "Fuel injection timing".to_string(),
                },
            ],
            scalars: vec![
                ScalarDefinition {
                    name: "Rev_Limiter".to_string(),
                    address: 0x50000,
                    byte_offset: 0,
                    byte_length: 2,
                    unit: "rpm".to_string(),
                    scale: 1.0,
                    offset: 0.0,
                    min_value: 4000.0,
                    max_value: 8500.0,
                    default_value: 7000.0,
                    description: "Coyote rev limiter".to_string(),
                },
            ],
            supported_os_ids: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_gm_p01_definition() {
        let def = definitions::gm_p01_ls1();
        assert_eq!(def.ecu_id, "gm_p01_512k");
        assert_eq!(def.tables.len(), 2);
        assert_eq!(def.scalars.len(), 2);
        assert_eq!(def.family, EcuFamily::GmP01Ls1Ls6);
    }
    
    #[test]
    fn test_gm_p59_definition() {
        let def = definitions::gm_p59_ls2();
        assert_eq!(def.ecu_id, "gm_p59_1m_ls2");
        assert_eq!(def.tables.len(), 2);
        assert_eq!(def.scalars.len(), 1);
        assert_eq!(def.family, EcuFamily::GmP59Ls2Ls3Ls7);
    }
    
    #[test]
    fn test_parser_basic() {
        let parser = EcuParser::new();
        let data = vec![0u8; 0x80000]; // 512KB dummy binary
        let def = definitions::gm_p01_ls1();
        
        // Create temp definition file
        let def_json = serde_json::to_string(&def).unwrap();
        let temp_path = std::path::PathBuf::from("/tmp/test_def.json");
        std::fs::write(&temp_path, &def_json).unwrap();
        
        let loaded = parser.load_definition(&temp_path).unwrap();
        assert_eq!(loaded.ecu_id, "gm_p01_512k");
        
        let _binary = parser.parse(data, "gm_p01_512k").unwrap();
        
        let _ = std::fs::remove_file(&temp_path);
    }
}

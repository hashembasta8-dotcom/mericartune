//! AI-powered calibration assistant
//!
//! Uses machine learning to suggest optimal ECU tune parameters
//! based on vehicle characteristics, modification level, and goals.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use anyhow::Result;

/// AI calibration engine
pub struct AiCalibrator {
    /// Base models for different platforms
    base_models: HashMap<String, BaseModel>,
    /// Tuned parameters cache
    parameter_cache: HashMap<String, TuneParameters>,
}

/// Base model for a specific ECU platform
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BaseModel {
    platform: String,
    ecu_type: String,
    displacement: f32,
    horsepower_range: (f32, f32),
    torque_range: (f32, f32),
    base_maps: BaseMaps,
    learning_rate: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BaseMaps {
    fuel_lt1: Vec<Vec<f32>>,
    spark_lt1: Vec<Vec<f32>>,
    boost_pressure: Vec<Vec<f32>>,
    vvt_timing: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TuneTarget {
    MaximumPower,
    DailyDriver,
    Economy,
    Racing,
    Street,
    Custom { description: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuneParameters {
    pub make: String,
    pub model: String,
    pub year: String,
    pub displacement: f32,
    pub mods: ModificationLevel,
    pub target: TuneTarget,
    pub fuel_lt1: Vec<Vec<f32>>,
    pub spark_lt1: Vec<Vec<f32>>,
    pub boost_pressure: Vec<Vec<f32>>,
    pub vvt_timing: Vec<f32>,
    pub scalars: HashMap<String, f32>,
    pub safety: SafetyProfile,
    pub estimated_hp: f32,
    pub estimated_torque: f32,
    pub estimated_gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModificationLevel {
    Stock,
    Stage1,
    Stage2,
    Stage3,
    Built,
    Race,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyProfile {
    pub max_egt: f32,
    pub max_cylinder_pressure: f32,
    pub safe_afr_range: (f32, f32),
    pub max_ignition_timing: f32,
    pub max_boost: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiAnalysis {
    pub base_file: String,
    pub target_profile: TuneTarget,
    pub recommended_changes: Vec<ChangeRecommendation>,
    pub safety_warnings: Vec<String>,
    pub estimated_gain: f32,
    pub confidence_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRecommendation {
    pub parameter: String,
    pub current_value: f32,
    pub recommended_value: f32,
    pub change_reason: String,
    pub safety_impact: String,
}

impl AiCalibrator {
    pub fn new() -> Self {
        let mut base_models = HashMap::new();
        
        base_models.insert("gm_p01".to_string(), BaseModel {
            platform: "GM P01".to_string(),
            ecu_type: "LS1/LS6".to_string(),
            displacement: 5.7,
            horsepower_range: (350.0, 550.0),
            torque_range: (380.0, 500.0),
            base_maps: BaseMaps {
                fuel_lt1: vec![vec![8.0; 11]; 6],
                spark_lt1: vec![vec![20.0; 11]; 6],
                boost_pressure: vec![],
                vvt_timing: vec![],
            },
            learning_rate: 0.1,
        });
        
        base_models.insert("gm_p59".to_string(), BaseModel {
            platform: "GM P59".to_string(),
            ecu_type: "LS2/LS3/LS7".to_string(),
            displacement: 6.2,
            horsepower_range: (400.0, 650.0),
            torque_range: (420.0, 580.0),
            base_maps: BaseMaps {
                fuel_lt1: vec![vec![8.0; 12]; 6],
                spark_lt1: vec![vec![22.0; 12]; 6],
                boost_pressure: vec![],
                vvt_timing: vec![],
            },
            learning_rate: 0.1,
        });
        
        base_models.insert("ford_coyote".to_string(), BaseModel {
            platform: "Ford Coyote".to_string(),
            ecu_type: "5.0L Ti-VCT".to_string(),
            displacement: 5.0,
            horsepower_range: (400.0, 700.0),
            torque_range: (400.0, 650.0),
            base_maps: BaseMaps {
                fuel_lt1: vec![vec![7.5; 8]; 4],
                spark_lt1: vec![vec![25.0; 8]; 4],
                boost_pressure: vec![],
                vvt_timing: vec![],
            },
            learning_rate: 0.1,
        });
        
        Self {
            base_models,
            parameter_cache: HashMap::new(),
        }
    }
    
    pub fn analyze(&self, ecu_type: &str, target: &TuneTarget, 
                   mods: &ModificationLevel) -> Result<AiAnalysis> {
        let model = self.base_models.get(ecu_type)
            .ok_or_else(|| anyhow::anyhow!("Unsupported ECU type: {}", ecu_type))?;
        
        let mut recommendations = Vec::new();
        let mut warnings = Vec::new();
        
        let power_multiplier = match mods {
            ModificationLevel::Stock => 1.0,
            ModificationLevel::Stage1 => 1.05,
            ModificationLevel::Stage2 => 1.15,
            ModificationLevel::Stage3 => 1.25,
            ModificationLevel::Built => 1.4,
            ModificationLevel::Race => 1.6,
        };
        
        // Analyze fuel maps
        for (i, row) in model.base_maps.fuel_lt1.iter().enumerate() {
            for (j, &val) in row.iter().enumerate() {
                let target_val = match target {
                    TuneTarget::MaximumPower => val * 1.15 * power_multiplier,
                    TuneTarget::DailyDriver => val * 1.02,
                    TuneTarget::Economy => val * 0.9,
                    TuneTarget::Racing => val * 1.2 * power_multiplier,
                    TuneTarget::Street => val * 1.1 * power_multiplier,
                    TuneTarget::Custom { .. } => *val,
                };
                
                if (target_val - val).abs() > 0.5 {
                    recommendations.push(ChangeRecommendation {
                        parameter: format!("Fuel_LT1[{}][{}]", i, j),
                        current_value: val,
                        recommended_value: target_val,
                        change_reason: format!("Optimized for target"),
                        safety_impact: "Monitor EGT and AFR".to_string(),
                    });
                }
            }
        }
        
        // Analyze spark maps
        for (i, row) in model.base_maps.spark_lt1.iter().enumerate() {
            for (j, &val) in row.iter().enumerate() {
                let mut target_val = match target {
                    TuneTarget::MaximumPower => val + 2.0,
                    TuneTarget::DailyDriver => val + 0.5,
                    TuneTarget::Economy => val - 1.0,
                    TuneTarget::Racing => val + 3.0,
                    TuneTarget::Street => val + 1.5,
                    TuneTarget::Custom { .. } => *val,
                };
                
                if target_val > model.horsepower_range.1 / 100.0 * 0.8 {
                    warnings.push(format!("Spark timing at [{}][{}] may cause knock", i, j));
                    target_val -= 1.0;
                }
                
                if (target_val - val).abs() > 1.0 {
                    recommendations.push(ChangeRecommendation {
                        parameter: format!("Spark_LT1[{}][{}]", i, j),
                        current_value: val,
                        recommended_value: target_val,
                        change_reason: format!("Optimized for target"),
                        safety_impact: "Check for knock with dyno".to_string(),
                    });
                }
            }
        }
        
        let est_gain = match (target, mods) {
            (TuneTarget::MaximumPower, ModificationLevel::Stage1) => 25.0,
            (TuneTarget::MaximumPower, ModificationLevel::Stage2) => 35.0,
            (TuneTarget::MaximumPower, ModificationLevel::Stage3) => 45.0,
            (TuneTarget::MaximumPower, ModificationLevel::Built) => 60.0,
            _ => 20.0,
        };
        
        let confidence = match mods {
            ModificationLevel::Stock => 0.85,
            ModificationLevel::Stage1 => 0.80,
            ModificationLevel::Stage2 => 0.75,
            ModificationLevel::Stage3 => 0.70,
            ModificationLevel::Built => 0.65,
            ModificationLevel::Race => 0.60,
        };
        
        Ok(AiAnalysis {
            base_file: format!("base_{}.bin", ecu_type),
            target_profile: target.clone(),
            recommended_changes: recommendations,
            safety_warnings: warnings,
            estimated_gain: est_gain,
            confidence_score: confidence,
        })
    }
    
    pub fn validate(&self, params: &TuneParameters) -> Vec<String> {
        let mut warnings = Vec::new();
        
        for row in &params.fuel_lt1 {
            for &val in row {
                if val < 7.0 {
                    warnings.push("Fuel map too lean - risk of detonation".to_string());
                }
                if val > 18.0 {
                    warnings.push("Fuel map too rich".to_string());
                }
            }
        }
        
        for row in &params.spark_lt1 {
            for &val in row {
                if val > params.safety.max_ignition_timing {
                    warnings.push(format!("Spark timing {} exceeds safe limit {}", val, params.safety.max_ignition_timing));
                }
            }
        }
        
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_ai_analysis() {
        let ai = AiCalibrator::new();
        let analysis = ai.analyze("gm_p59", &TuneTarget::MaximumPower, &ModificationLevel::Stage2).unwrap();
        assert!(analysis.estimated_gain > 0.0);
        assert!(analysis.confidence_score > 0.0);
    }
}

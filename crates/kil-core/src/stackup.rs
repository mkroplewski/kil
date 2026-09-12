//! Board-wide layer order and optional fabrication dimensions.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Stackup {
    /// Even copper layer count. Inner layers are named In1.Cu, In2.Cu, and so on.
    #[schemars(range(min = 2, max = 32))]
    pub layers: u8,
    pub thickness: f64,
    /// Uniform copper thickness in mm.
    pub copper_thickness: f64,
    /// Optional fabrication specification, in order between adjacent copper layers.
    pub dielectrics: Vec<Dielectric>,
}
impl Default for Stackup {
    fn default() -> Self {
        Self {
            layers: 2,
            thickness: 1.6,
            copper_thickness: 0.035,
            dielectrics: vec![],
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dielectric {
    pub thickness: f64,
    #[serde(default = "material")]
    pub material: String,
    #[serde(default = "epsilon")]
    pub epsilon_r: f64,
}
fn material() -> String {
    "FR4".into()
}
fn epsilon() -> f64 {
    4.5
}
impl Stackup {
    pub fn copper_layers(&self) -> Vec<String> {
        let mut layers = vec!["F.Cu".into()];
        layers.extend((1..self.layers.saturating_sub(1)).map(|i| format!("In{i}.Cu")));
        layers.push("B.Cu".into());
        layers
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(2..=32).contains(&self.layers) || !self.layers.is_multiple_of(2) {
            return Err("copper layer count must be even, between 2 and 32".into());
        }
        if !self.thickness.is_finite()
            || !self.copper_thickness.is_finite()
            || self.copper_thickness <= 0.0
            || self.thickness <= f64::from(self.layers) * self.copper_thickness
        {
            return Err("board thickness must exceed the total positive copper thickness".into());
        }
        if !self.dielectrics.is_empty() {
            if self.dielectrics.len() != usize::from(self.layers - 1) {
                return Err("specify one dielectric between each adjacent copper layer".into());
            }
            if self.dielectrics.iter().any(|d| {
                !d.thickness.is_finite()
                    || d.thickness <= 0.0
                    || !d.epsilon_r.is_finite()
                    || d.epsilon_r < 1.0
                    || d.material.is_empty()
            }) {
                return Err(
                    "dielectrics require positive thickness, material, and epsilon_r >= 1".into(),
                );
            }
            let total = self.dielectrics.iter().map(|d| d.thickness).sum::<f64>()
                + f64::from(self.layers) * self.copper_thickness;
            if (total - self.thickness).abs() > 1e-6 {
                return Err("copper and dielectric thicknesses must sum to board thickness".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layer_counts_and_fabrication_dimensions_are_checked() {
        let mut stack = Stackup {
            layers: 4,
            ..Default::default()
        };
        assert_eq!(stack.copper_layers(), ["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"]);
        assert!(stack.validate().is_ok());
        for layers in [0, 1, 3, 33] {
            stack.layers = layers;
            assert!(stack.validate().is_err());
        }
        stack.layers = 4;
        stack.dielectrics = vec![Dielectric {
            thickness: 1.46,
            material: material(),
            epsilon_r: epsilon(),
        }];
        assert!(stack.validate().is_err());
        stack.dielectrics = [0.2, 1.06, 0.2]
            .into_iter()
            .map(|thickness| Dielectric {
                thickness,
                material: material(),
                epsilon_r: epsilon(),
            })
            .collect();
        assert!(stack.validate().is_ok());
        stack.dielectrics[0].thickness += 0.01;
        assert!(stack.validate().is_err());
    }
}

//! Rarity weighting (IDF) and cosine similarity over feature tokens.

use std::collections::BTreeMap;

use crate::features::Features;

/// How many movies in a reference set contain each feature token.
#[derive(Debug, Clone, Default)]
pub struct Idf {
    doc_count: u32,
    doc_freq: BTreeMap<String, u32>,
}

impl Idf {
    pub fn from_documents<'a>(documents: impl IntoIterator<Item = &'a Features>) -> Self {
        let mut idf = Self::default();
        for doc in documents {
            idf.doc_count += 1;
            for token in doc.keys() {
                *idf.doc_freq.entry(token.clone()).or_default() += 1;
            }
        }
        idf
    }

    /// Smoothed inverse document frequency: `ln((1 + N) / (1 + df)) + 1`.
    ///
    /// Never below 1, so common tokens still count a little, and tokens missing
    /// from the reference set get the highest weight.
    pub fn weight(&self, token: &str) -> f32 {
        let df = self.doc_freq.get(token).copied().unwrap_or(0);
        ((1.0 + self.doc_count as f32) / (1.0 + df as f32)).ln() + 1.0
    }
}

/// A unit-length TF-IDF vector. Empty when the movie has no usable features.
#[derive(Debug, Clone, Default)]
pub struct Vector(BTreeMap<String, f32>);

impl Vector {
    /// Weights each token by field importance × rarity, then scales to unit length.
    pub fn new(features: &Features, idf: &Idf) -> Self {
        let mut values: BTreeMap<String, f32> = features
            .iter()
            .map(|(token, weight)| (token.clone(), weight * idf.weight(token)))
            .collect();
        let norm = values.values().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            values.values_mut().for_each(|v| *v /= norm);
        } else {
            values.clear();
        }
        Self(values)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Cosine similarity, from 0.0 (nothing shared) to 1.0 (identical).
    pub fn cosine(&self, other: &Vector) -> f32 {
        let dot: f32 = self
            .0
            .iter()
            .filter_map(|(token, a)| other.0.get(token).map(|b| a * b))
            .sum();
        dot.clamp(0.0, 1.0)
    }

    /// Tokens in both vectors with their share of the cosine, largest first.
    pub fn shared(&self, other: &Vector) -> Vec<(&str, f32)> {
        let mut shared: Vec<_> = self
            .0
            .iter()
            .filter_map(|(token, a)| other.0.get(token).map(|b| (token.as_str(), a * b)))
            .collect();
        shared.sort_by(|x, y| y.1.total_cmp(&x.1).then_with(|| x.0.cmp(y.0)));
        shared
    }
}

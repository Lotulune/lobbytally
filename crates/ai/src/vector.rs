use crate::error::AiError;

/// Decode little-endian float32 embedding blob and validate dimensions.
pub fn decode_f32_le(blob: &[u8], expected_dimensions: usize) -> Result<Vec<f32>, AiError> {
    let expected_bytes = expected_dimensions
        .checked_mul(4)
        .ok_or_else(|| AiError::InvalidOutput("embedding dimensions overflow".into()))?;
    if blob.len() != expected_bytes {
        return Err(AiError::InvalidOutput(format!(
            "embedding blob length {} != dimensions {expected_dimensions} * 4",
            blob.len()
        )));
    }
    let mut out = Vec::with_capacity(expected_dimensions);
    for chunk in blob.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}

pub fn encode_f32_le(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub fn l2_normalize(vector: &mut [f32]) {
    let mut sum = 0.0f64;
    for value in vector.iter() {
        sum += f64::from(*value) * f64::from(*value);
    }
    if sum <= f64::EPSILON {
        return;
    }
    let norm = sum.sqrt() as f32;
    for value in vector.iter_mut() {
        *value /= norm;
    }
}

/// Cosine similarity. Returns 0 for empty or mismatched dimensions.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = f64::from(*x);
        let yf = f64::from(*y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na <= f64::EPSILON || nb <= f64::EPSILON {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())).clamp(-1.0, 1.0)
}

/// Reciprocal Rank Fusion over multiple ranked id lists (1-based ranks).
pub fn reciprocal_rank_fusion(ranked_lists: &[Vec<u32>], k: u32) -> Vec<(u32, f64)> {
    use std::collections::HashMap;
    let mut scores: HashMap<u32, f64> = HashMap::new();
    let k = f64::from(k.max(1));
    for list in ranked_lists {
        for (idx, id) in list.iter().enumerate() {
            let rank = (idx + 1) as f64;
            *scores.entry(*id).or_insert(0.0) += 1.0 / (k + rank);
        }
    }
    let mut items: Vec<(u32, f64)> = scores.into_iter().collect();
    items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    items
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_blob() {
        let v = vec![0.5f32, -1.0, 2.0];
        let blob = encode_f32_le(&v);
        let decoded = decode_f32_le(&blob, 3).unwrap();
        assert_eq!(decoded, v);
    }

    #[test]
    fn rejects_bad_blob_length() {
        assert!(decode_f32_le(&[0, 1, 2], 2).is_err());
    }

    #[test]
    fn cosine_identical_is_one() {
        let a = [1.0f32, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rrf_prefers_shared_top() {
        let fused = reciprocal_rank_fusion(&[vec![1, 2, 3], vec![2, 1, 4]], 60);
        // 1 and 2 appear in both lists at top ranks and outrank tail ids.
        let top_two: std::collections::HashSet<u32> = fused.iter().take(2).map(|(id, _)| *id).collect();
        assert!(top_two.contains(&1) && top_two.contains(&2));
        assert!(fused[0].1 >= fused.last().unwrap().1);
        assert!(fused.iter().find(|(id, _)| *id == 3).unwrap().1 < fused[0].1);
    }
}

//! t-SNE for the sound map: sounds that sound alike end up close together. Attraction uses each
//! point's nearest neighbours; repulsion is computed exactly, so it is meant for a few thousand
//! points at most.

use rayon::prelude::*;

/// 2-D positions for unit-length vectors `x`, starting from `init`.
pub fn tsne(x: &[Vec<f32>], init: &[[f64; 2]], perplexity: f64, iters: usize) -> Vec<[f64; 2]> {
    let n = x.len();
    let k = ((3.0 * perplexity) as usize).min(n - 1).max(1);
    // Nearest neighbours by squared distance (for unit vectors, 2 - 2 cos).
    let neigh: Vec<Vec<(usize, f64)>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut d: Vec<(usize, f64)> = (0..n)
                .filter(|&j| j != i)
                .map(|j| (j, x[i].iter().zip(&x[j]).map(|(a, b)| ((a - b) * (a - b)) as f64).sum::<f64>()))
                .collect();
            d.select_nth_unstable_by(k - 1, |a, b| a.1.total_cmp(&b.1));
            d.truncate(k);
            d
        })
        .collect();
    // Each point's Gaussian width, found by bisection so its entropy matches the perplexity.
    let target = perplexity.ln();
    let cond: Vec<Vec<(usize, f64)>> = neigh
        .par_iter()
        .map(|nb| {
            let (mut lo, mut hi, mut beta) = (0.0f64, f64::INFINITY, 1.0f64);
            let dmin = nb.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
            let mut p = vec![0.0; nb.len()];
            for _ in 0..64 {
                let mut sum = 0.0;
                for (pi, (_, d)) in p.iter_mut().zip(nb) {
                    *pi = (-(d - dmin) * beta).exp();
                    sum += *pi;
                }
                let sum = sum.max(1e-12);
                let mut h = 0.0;
                for (pi, (_, d)) in p.iter_mut().zip(nb) {
                    *pi /= sum;
                    h += *pi * (d - dmin) * beta;
                }
                let h = h + sum.ln();
                if (h - target).abs() < 1e-5 {
                    break;
                }
                if h > target {
                    lo = beta;
                    beta = if hi.is_infinite() { beta * 2.0 } else { (beta + hi) / 2.0 };
                } else {
                    hi = beta;
                    beta = (beta + lo) / 2.0;
                }
            }
            nb.iter().zip(p).map(|((j, _), pv)| (*j, pv)).collect()
        })
        .collect();
    // Symmetric joint probabilities, as sparse rows.
    let mut rows: Vec<std::collections::HashMap<usize, f64>> = vec![Default::default(); n];
    for (i, r) in cond.iter().enumerate() {
        for &(j, v) in r {
            *rows[i].entry(j).or_default() += v / (2.0 * n as f64);
            *rows[j].entry(i).or_default() += v / (2.0 * n as f64);
        }
    }
    let p: Vec<Vec<(usize, f64)>> = rows.into_iter().map(|r| r.into_iter().collect()).collect();

    let mut y: Vec<[f64; 2]> = init.to_vec();
    let mut vel = vec![[0.0f64; 2]; n];
    let mut gains = vec![[1.0f64; 2]; n];
    let lr = (n as f64 / 12.0 / 4.0).max(50.0);
    for it in 0..iters {
        let exag = if it < 250 { 12.0 } else { 1.0 };
        let mom = if it < 250 { 0.5 } else { 0.8 };
        let z: f64 = y.par_iter().enumerate().map(|(i, yi)| y.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, yj)| 1.0 / (1.0 + (yi[0] - yj[0]).powi(2) + (yi[1] - yj[1]).powi(2))).sum::<f64>()).sum();
        let grad: Vec<[f64; 2]> = (0..n)
            .into_par_iter()
            .map(|i| {
                let yi = y[i];
                let mut g = [0.0f64; 2];
                for (j, yj) in y.iter().enumerate() {
                    if j == i {
                        continue;
                    }
                    let (dx, dy) = (yi[0] - yj[0], yi[1] - yj[1]);
                    let q = 1.0 / (1.0 + dx * dx + dy * dy);
                    g[0] -= q * q / z * dx;
                    g[1] -= q * q / z * dy;
                }
                for &(j, pij) in &p[i] {
                    let (dx, dy) = (yi[0] - y[j][0], yi[1] - y[j][1]);
                    let q = 1.0 / (1.0 + dx * dx + dy * dy);
                    g[0] += exag * pij * q * dx;
                    g[1] += exag * pij * q * dy;
                }
                [4.0 * g[0], 4.0 * g[1]]
            })
            .collect();
        for i in 0..n {
            for d in 0..2 {
                gains[i][d] = if (grad[i][d] > 0.0) != (vel[i][d] > 0.0) { gains[i][d] + 0.2 } else { (gains[i][d] * 0.8).max(0.01) };
                vel[i][d] = mom * vel[i][d] - lr * gains[i][d] * grad[i][d];
                y[i][d] += vel[i][d];
            }
        }
    }
    y
}

#[cfg(test)]
mod tests {
    #[test]
    fn separates_two_clusters() {
        // Two tight groups of unit vectors in different directions must land apart.
        let mut x = Vec::new();
        for g in 0..2 {
            for k in 0..20 {
                let mut v = vec![0.0f32; 8];
                v[g * 4] = 1.0;
                v[g * 4 + 1] = 0.05 * k as f32 / 20.0;
                let n = v.iter().map(|a| a * a).sum::<f32>().sqrt();
                x.push(v.iter().map(|a| a / n).collect());
            }
        }
        let init: Vec<[f64; 2]> = (0..40).map(|i| [(i as f64 * 0.37).sin() * 1e-4, (i as f64 * 0.71).cos() * 1e-4]).collect();
        let y = super::tsne(&x, &init, 10.0, 500);
        let c = |r: std::ops::Range<usize>| {
            let n = r.len() as f64;
            r.fold([0.0, 0.0], |a, i| [a[0] + y[i][0] / n, a[1] + y[i][1] / n])
        };
        let (a, b) = (c(0..20), c(20..40));
        let between = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
        let spread = (0..20).map(|i| ((y[i][0] - a[0]).powi(2) + (y[i][1] - a[1]).powi(2)).sqrt()).fold(0.0, f64::max);
        assert!(between > 2.0 * spread, "clusters {between} apart, spread {spread}");
    }
}

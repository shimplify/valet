use crate::state::*;
use linfa::DatasetBase;
use linfa::traits::{Fit, Predict};
use linfa_clustering::KMeans;
use ndarray::{Array2, Axis, array};
use rand::SeedableRng;
use rand_distr::{Distribution, Normal};
use rand_xoshiro::Xoshiro256Plus;

// pub static PATH_TO_UID: LazyLock<> =
// LazyLock::new(|| Arc::new(DashMap::new()));

pub fn get_stream(filename: String, _flags: i32, _mode: libc::mode_t) -> Stream {
    let seed = 42;
    let mut rng = Xoshiro256Plus::seed_from_u64(seed);

    // `expected_centroids` has shape `(n_centroids, n_features)`
    // i.e. three points in the 2-dimensional plane
    let expected_centroids = array![[300., 330.], [340., 350.]];

    // Generate blob data manually instead of using linfa-datasets
    let n_samples = 100;
    let n_features = 2;
    let n_centroids = expected_centroids.len_of(Axis(0));
    let samples_per_centroid = n_samples / n_centroids;
    let std_dev = 10.0;

    let mut data = Array2::<f64>::zeros((n_samples, n_features));
    for (i, centroid) in expected_centroids.axis_iter(Axis(0)).enumerate() {
        let start_idx = i * samples_per_centroid;
        let end_idx = if i == n_centroids - 1 {
            n_samples
        } else {
            (i + 1) * samples_per_centroid
        };

        for j in start_idx..end_idx {
            for k in 0..n_features {
                let normal = Normal::new(centroid[k], std_dev).unwrap();
                data[[j, k]] = normal.sample(&mut rng);
            }
        }
    }

    let observations = DatasetBase::from(data.clone());
    // Let's generate a synthetic dataset: three blobs of observations
    // (100 points each) centered around our `expected_centroids`
    let n_clusters = expected_centroids.len_of(Axis(0));
    let model = KMeans::params_with_rng(n_clusters, rng.clone())
        .tolerance(1e-2)
        .fit(&observations)
        .expect("KMeans fitted");
    let mut val = 0;
    if let Some(pos) = filename.find('.') {
        let after_dot = &filename[pos + 1..];
        val = after_dot.chars().map(|c| c as u32).sum();
    }
    let new_observation = DatasetBase::from(array![[val as f64, val as f64]]);
    let dataset = model.predict(new_observation);
    let closest_centroid = &model.centroids().index_axis(Axis(0), dataset.targets()[0]);
    shimmer_info!("{filename}: predicted {closest_centroid}");

    if dataset.targets()[0] == 0 {
        Stream::LOG
    } else {
        Stream::SSTable
    }
}

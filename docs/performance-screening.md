# Training and blocking experiments

The million-record quality screen selects the 1M EM pair target for controlled
timing. Removing or narrowing the broad prediction rule exceeds the recall budget.
The vector smoke test does not justify production activation. Production defaults
remain unchanged; the [experiment guide](performance-experiments.md) describes
reproduction and the three-repeat, 10M promotion requirement.
The subsequent [single-load confirmation](performance-training-full-load.md)
measures the training candidate with other workloads stopped.

## Million-record quality screen

All six arms use the same `hard-v1` corpus, generator seed 20260101 and reference
configuration, followed by the same 10,000-record delivery and a correction without
retraining. Each initial load includes ingestion, standardization, training,
matching, reconciliation and golden assembly. Input hashes match across arms.
The [machine-readable measurements](measurements/performance-screening-20260923.json)
retain raw quality counts, image/source identities, block statistics, output
validation and each scoring generation.

These runs overlapped integration tests. Their timings are deliberately excluded
from this report and from performance claims. This is one tuning seed, not a
held-out or real-tenant accuracy assessment.

| Arm | Candidate pairs | Blocking recall | Cluster precision | Cluster recall |
|---|---:|---:|---:|---:|
| Current, uncapped EM | 5,408,610 | 94.90525% | 75.62817% | 88.851375% |
| EM target 1M | 5,408,610 | 94.90525% | 75.63462% | 88.853000% |
| EM target 5M | 5,408,610 | 94.90525% | 75.62817% | 88.851375% |
| EM target 10M | 5,408,610 | 94.90525% | 75.62817% | 88.851375% |
| Narrow name/postcode by given initial | 1,541,180 | 94.19113% | 75.67439% | 88.447750% |
| Remove name/postcode | 1,187,391 | 93.12075% | 76.21645% | 88.141750% |

The 1M training target finds 13 more true cluster pairs and 76 fewer false pairs
than uncapped training in the initial load. Precision and recall also improve
slightly after the delivery and correction. The 5M and 10M targets reproduce the
uncapped quality counts in all three workloads.

Narrowing the prediction rule removes 71.5% of candidates but loses 0.403625
percentage points of cluster recall. Removing it removes 78.0% of candidates but
loses 0.709625 points. Both exceed the 0.01-point budget despite higher precision.
Changing this prediction rule does not remove the separate surname/postcode EM
training cost.

## Provider smoke test

The complete pipeline runs on 1,000 hard-profile records, then receives 50 more.
At that 1,050-record state, all candidate-key arms use the same learned model and
frozen TF snapshot. Generator seed is 42; LSH seed is 20260101. The optional BGE
provider runs through CPU ONNX with baked, checksum-verified model files. A separate
inference check succeeds with Docker networking disabled.

| Arm | Candidates | Blocking recall | Cluster precision | Cluster recall |
|---|---:|---:|---:|---:|
| Current | 1,065 | 95.15789% | 78.03571% | 92.00000% |
| MinHash added | 1,177 | 96.42105% | 78.03571% | 92.00000% |
| MinHash replaces name/postcode | 1,177 | 96.42105% | 78.03571% | 92.00000% |
| BGE added | 1,974 | 96.31579% | 77.20848% | 92.00000% |
| BGE replaces name/postcode | 1,969 | 96.21053% | 77.18833% | 91.89474% |

MinHash retrieves more true pairs without improving cluster recall on this sample;
those records already connect through other edges. BGE increases candidates and
reduces precision. This sample validates the implementation path, not throughput
or large-scale accuracy. No production vector store, blocking activation or
coherence scorer is enabled by these results.

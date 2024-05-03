# S3 Express Benchmark

This directory contains S3 Express One Zone benchmarks that measure end-to-end throughput when using the AWS Rust SDK to put, get, and delete objects to/from S3 Express One Zone buckets. We use [`Criterion`](https://github.com/bheisler/criterion.rs) for benchmarks. A sibling directory `s3-throughput` also measures throughput to put and get objects to/from S3 but currently does not support generating benchmark results with a given confidence interval, which is why we have this separate benchmark suite.

Performance numbers will vary depending on the benchmarking environment, but relative performance should still be accurate (i.e. regular S3 bucket vs. S3 Express bucket or comparing with a previous release of the Rust SDK).

## Benchmark targets
- `get_object`: Invoke `GetObject` the specified number of times (20 by default) against a given set of buckets, using both 64KB and 1MB objects.
- `put_object`: Invoke `PutObject` the specified number of times (20 by default) against a given set of buckets, using both 64KB and 1MB objects.
- `put_get_delete`: `PutObject`, `GetObject`, and `DeleteObject` using sequential invocations (20 by default) of operations across different buckets, switching buckets on every request and using both 64KB and 1MB objects.
- `concurrent_put_get`: Schedule the equal number of async tasks of `PutObject` (20 by default) to different buckets, wait for completion, then schedule the equal number of async tasks of `GetObject` to different buckets, and wait for completion, using the 64KB objects.

## Running benchmarks
Example of running the `put_object` benchmark in local dev environment:

```bash
export BUCKETS=test0--usw2-az1--x-s3,test1--usw2-az1--x-s3
cargo bench --bench put_object
```
To configure how the benchmark is run, set the following environment variables:
#### required
- `BUCKETS`: a list of comma separated bucket names

#### optional
- `CONFIDENCE_LEVEL`: the confidence level for benchmarks in a group (0.99 by default)
- `NUMBER_OF_ITERATIONS`: the number of times a set of operations runs for measurement (20 by default)
- `SAMPLE_SIZE`: the size of the sample for benchmarks in a group (10 by default)

### Flamegraph generation
Use [`flamegraph`](https://github.com/flamegraph-rs/flamegraph) to generate one for a target bench, for instance:
```bash
export BUCKETS=test0--usw2-az1--x-s3,test1--usw2-az1--x-s3
cargo flamegraph --bench put_get_delete -- --bench
```

The resulting flamegraph `flamegraph.svg` should be generated in the current directory.


## Limitation
Benchmarks currently measure end-to-end throughput of operations, including both the Rust SDK latency and the server side latency. To detect regressions in the Rust SDK reliably, we should only capture the time taken before sending a request and after receiving a response.

## Baseline
As of b172a1e, here are performance numbers for the targets `get_object` and `put_object` run against a single express bucket within the `us-west-2` region (showing additional outputs to display config parameters). The benchmarks are measured on Amazon Linux 2 x86_64 5.10 Kernel with a host type c5.4xlarge.
```
[src/lib.rs:30] sample_size = 10
[src/lib.rs:14] confidence_level = 0.99
[src/lib.rs:23] number_of_iterations = 20
measuring 20 of GetObject against [
    "s3express-rust-sdk-benchmark--usw2-az1--x-s3",
], switching buckets on every operation if more than one bucket is specified
get_object/size/65536   time:   [304.20 ms 311.62 ms 317.62 ms]
get_object/size/1048576 time:   [283.94 ms 289.16 ms 293.42 ms]

[src/lib.rs:30] sample_size = 10
[src/lib.rs:14] confidence_level = 0.99
[src/lib.rs:23] number_of_iterations = 20
measuring 20 of PutObject against [
    "s3express-rust-sdk-benchmark--usw2-az1--x-s3",
], switching buckets on every operation if more than one bucket is specified
put_object/size/65536   time:   [163.01 ms 172.76 ms 185.16 ms]
put_object/size/1048576 time:   [356.49 ms 368.64 ms 383.51 ms]
```

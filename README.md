# SKOPE

Skope is an ultra-fast and flexible tool designed for benchmarking any application. It provides detailed reporting and a UI for viewing differences in performance between different benchmarks.

<img width="2284" height="1209" alt="image" src="https://github.com/user-attachments/assets/1de7695b-73aa-47e8-bcfe-89fc8ace7e4d" />


## Installation

### Build from Source

1. **Install Rust**  
   If you don't have Rust installed, follow the official installation guide:  
   [Rust Installation Guide](https://www.rust-lang.org/learn/get-started)

2. **Clone the Repository**

   ```bash
   git clone git@github.com:richardhapb/skope.git
   ```

3. **Build the Project**

   Navigate into the project directory and build the release version:

   ```bash
   cd skope
   cargo build --release
   ```

4. **Optional - Create a Symlink for Easy Usage**

   To make `skope` easily accessible from anywhere on your system, you can create a symbolic link:

   ```bash
   sudo ln -sf $(pwd)/target/release/skope /usr/local/bin
   ```

## Usage

```text
Usage: skope [OPTIONS] <COMMAND>

Commands:
  runner  Execute the application using a script
  server  Start a server to listen for /start and /stop signals, generating reports for each captured difference
  agg     Start a server to store and aggregate application data
  diff    Compare two tagged captures to view differences
  help    Print this message or the help for a given command

Options:
  -h, --host <HOST>    The host that Skope will listen to
  -p, --port <PORT>    The port that Skope will listen to
  -h, --help           Print help
  -V, --version        Print version
```

### Binding

By default, Skope listens on `localhost` at port `9001`. You can customize these settings by passing the `--host` and `--port` arguments as CLI options, or by setting the `SKOPE_HOST` and `SKOPE_PORT` environment variables.

### Reports

The `json` reports will be saved inside the `.skope` directory from where you are executing the application.

### Runner Mode

In this mode, `Skope` will execute a specified bash script to run your target application and collect benchmarking data. The `--tag` flag is required to assign a name to the benchmark capture, and `--script` indicates the path of the bash script.

Example of a bash script:

```bash
#!/usr/bin/env bash

echo "Sending START signal..."
curl -s http://localhost:9001/start
echo ""

# Your application logic or requests go here

echo "Sending STOP signal..."
curl -s http://localhost:9001/stop
echo "Tearing down..."
```

Example output:

```json
{
  "name": "test1",
  "module": null,
  "timestamp": 1751493411,
  "system_manager": {
    "memory_usage": 62.375,
    "cpu_usage": 0.000024795532
  },
  "exec_time": 10.127969
}
```

### Server Mode

In `server` mode, `Skope` listens for `/start?name=<app_name>` and `/stop` requests. You can capture multiple benchmarks without restarting Skope by using different names for each `/start` request. The profiling will stop when a `SIGTERM` signal (e.g., `Ctrl-C`) is sent.

### Aggregate Mode (agg)

This mode allows you to aggregate multiple benchmarking results. You can send `POST` requests to the root `/` endpoint to aggregate the data. The two generated reports are:

- **`report_agg.json`**: Contains aggregated data grouped by application name.
- **`report_apps.json`**: Contains raw data for individual application executions.

Example of `report_agg.json`:

```json
{
  "chat": {
    "total_execs": 10,
    "total_exec_time": 1.000882708,
    "total_memory_usage": 0.0,
    "avg_exec_memory": 0.0,
    "avg_exec_time": 0.1000882708
  },
  "home": {
    "total_execs": 1,
    "total_exec_time": 0.02810204,
    "total_memory_usage": 0.0,
    "avg_exec_memory": 0.0,
    "avg_exec_time": 0.02810204
  }
}
```

Example of `report_apps.json`:

```json
[
  {
    "name": "app1",
    "module": "my_module",
    "timestamp": 1749853300.0,
    "exec_memory_usage": 0.0,
    "total_memory_global": 225.45703,
    "exec_time": 0.006235625
  },
  {
    "name": "app2",
    "module": "some_module",
    "timestamp": 1749853300.0,
    "exec_memory_usage": 0.125,
    "total_memory_global": 225.45703,
    "exec_time": 0.018422084
  }
]
```

### Diff Mode

Use this mode to compare two previously captured benchmarks and view the differences.

Example:

```bash
skope diff --base test1 --head test2
```

Example output:

```json
{
  "name": "test1-test2",
  "module": null,
  "timestamp": 1751493411,
  "system_manager": {
    "memory_usage": -53.984375,
    "cpu_usage": 0.0000038146973
  },
  "exec_time": -0.9409857
}
```

## Contributing

We welcome contributions! If you find a bug, have a feature suggestion, or want to contribute to the codebase, please open an issue or submit a pull request.

## License

This project is licensed under the MIT License.


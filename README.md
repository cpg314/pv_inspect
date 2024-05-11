## pv_inspect

Mount a Kubernetes PersistentVolumeClaim volume on a new pod, shell into it, and port-forward if desired. Delete the pod when done.

Three pod templates are provided (see the `templates` directory):

- `sleep`: Debian container, with a sleep command.
- `miniserve`: Container with a [miniserve](https://github.com/svenstaro/miniserve) HTTP server of the volume contents, port-forwarded on port 8080.
- `samba`: Container with a [Samba server](https://hub.docker.com/r/dperson/samba) sharing the volume contents, automatically bound on port 8080.

Custom templates can be passed with the `--template-yaml` option. A `data` volume/volume mount is automatically added and mounted on `/data`.

### Installation

See the packages on the [releases page](https://github.com/cpg314/pv_inspect/releases).

Alternatively, compile with cargo:

```console
$ cargo install --git https://github.com/cpg314/pv_inspect
```

### Usage

```
Mount a PVC on a new pod, shell into it, and port-forward if desired

Usage: pv_inspect [OPTIONS] [NAME]

Arguments:
  [NAME]
          Name of the PVC to inspect

Options:
  -n, --namespace <NAMESPACE>
          [default: default]

      --template <TEMPLATE>
          [default: miniserve]

          Possible values:
          - miniserve: Shell and miniserve port-forwarded on 8080:8080
          - sleep:     A pod that sleeps
          - samba:     Shell and Samba mount port-forwarded on 8080:445

      --template-yaml <TEMPLATE_YAML>
          Alternatively, path to a custom pod template

      --port <PORT>
          Bind a port on the pod. Format: host:pod

  -h, --help
          Print help (see a summary with '-h')
```

#### Examples

With the default template (`miniserve`):

```console
$ pv_inspect -n my-namespace my-pvc
[INFO  pv_inspect] Creating pod
[INFO  pv_inspect] Waiting for pod "pvc-inspect-my-pvc-gqnp9"
[INFO  pv_inspect] Pod created
[INFO  pv_inspect] Starting port forwarding on port 8080:8080
[INFO  pv_inspect] Miniserve on http://localhost:8080
[INFO  pv_inspect] Connecting to pod. Type Control+D to exit the shell
root@pvc-inspect-my-pvc-gqnp9:/data# ls
folder1 folder2 README
root@pvc-inspect-my-pvc-gqnp9:/data#
[INFO  pv_inspect] Stopping port forwarding
[INFO  pv_inspect] Deleting pod
[INFO  pv_inspect] Waiting for deletion
[INFO  pv_inspect] Pod deleted
```

With the `samba` template:

```console
$ pv_inspect -n my-namespace my-pvc --template samba
[INFO  pv_inspect] Creating pod
[INFO  pv_inspect] Waiting for pod "pvc-inspect-my-pvc-gqnp9"
[INFO  pv_inspect] Pod created
[INFO  pv_inspect] Starting port forwarding on port 8080:8080
[INFO  pv_inspect] Miniserve on http://localhost:8080
[INFO  pv_inspect] Connecting to pod. Type Control+D to exit the shell
```

```console
$ sudo mount -t cifs //127.0.0.1/public samba -o port=8080 -o password=""
$ ls samba
folder1 folder2 README
```

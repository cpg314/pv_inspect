# pv_inspect

Small utility to "shell into" a Kubernetes volume and mount it locally if desired.

This allows easily inspecting or accessing volumes, even when they are not already mounted.

This works by automatically:

- Creating a new pod with the volume attached.
- Opening a shell into the pod.
- Optionally mounting the volume locally via SSHFS, after port forwarding on an OpenSSH server running on the pod.
- Deleting the pod when the tool is exited.

## Installation

See the packages on the [releases page](https://github.com/cpg314/pv_inspect/releases).

Alternatively, compile with cargo:

```console
$ cargo install --git https://github.com/cpg314/pv_inspect
```

## Usage

```
Mount a PVC on a new pod, shell into it, and mount it (via SSHFS) if desired

Usage: pv_inspect [OPTIONS] [NAME]

Arguments:
  [NAME]  Name of the PVC to inspect. If not provided, a list will be shown.

Options:
  -n, --namespace <NAMESPACE>    [default: default]
  -m, --mountpoint <MOUNTPOINT>
      --rw                       Mount the volume in read/write mode rather than read only
      --nowait                   Do not wait until the pod has been deleted
      --cleanup                    Cleanup stale pv_inspect pods and exit
      --cleanup-min <CLEANUP_MIN>  Age in minutes to cleanup pods [default: 240]
  -h, --help                     Print help
  -V, --version                  Print version
```

For example:

```console
$ pv_inspect -n mynamespace --rw mypvc
$ # Mount locally
$ pv_inspect -n mynamespace --rw -m ~/mounts/volume mypvc
```

### As a `k9s` plugin

If you use the [k9s Kubernetes TUI](https://k9scli.io/), you can install `pv_inspect` as a plugin by editing your plugins configuration (see the output of `k9s info`) as follows:

```yaml
plugins:
  pv_inspect:
    shortCut: p
    description: pv_inspect
    scopes:
      - pvc
    command: pv_inspect
    args:
      - -n
      - $NAMESPACE
      - $NAME
```

When viewing `PersistentVolumeClaims`, the `p` key (or any other you might choose) will launch `pv_inspect`:

![k9s screenshot](k9s.png)

## Cleanup

The `--cleanup` command allows delete `pv_inspect` dangling pods, due to the client aborting before deletion. It can also be executed on the cluster as a cronjob, see [`cleanup_job.yaml`](cleanup_job.yaml).

## TODO

- `rsync`-style subcommand.

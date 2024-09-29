use std::io::BufRead;
use std::net::TcpListener;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use futures::StreamExt;
use k8s_openapi::api::core::v1::{
    EnvVar, PersistentVolumeClaim, PersistentVolumeClaimVolumeSource, Pod, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::Metadata;
use kube::api::{Api, AttachParams, ListParams};
use kube::core::PartialObjectMetaExt;
use kube::runtime::conditions::is_deleted;
use kube::runtime::wait::{await_condition, Condition};
use log::*;
use tokio::io::AsyncWriteExt;

const LABEL_KEY: &str = "pv-inspect";
const LABEL_DELETE: &str = "0";

/// Mount a PVC on a new pod, shell into it, and mount it (via SSHFS) if desired.
#[derive(Parser)]
#[clap(version)]
struct Flags {
    #[clap(long, short, default_value = "default")]
    namespace: String,
    /// Name of the PVC to inspect. If not provided, a list will be shown.
    name: Option<String>,
    #[clap(long, short)]
    mountpoint: Option<PathBuf>,
    /// Mount the volume in read/write mode rather than read only.
    #[clap(long)]
    rw: bool,
    /// Do not wait until the pod has been deleted.
    #[clap(long)]
    nowait: bool,
    /// Cleanup stale pv_inspect pods and exit
    #[clap(long)]
    cleanup: bool,
    /// Age in minutes to cleanup pods
    #[clap(long,default_value_t=4*60)]
    cleanup_min: u64,
}

#[derive(tabled::Tabled)]
struct Pvc {
    name: String,
    creation: String,
    size: String,
}

async fn main_impl() -> anyhow::Result<()> {
    let args = Flags::parse();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut config = kube::Config::infer().await?;
    info!("Connecting to cluster {}", config.cluster_url);
    config.read_timeout = None;
    config.write_timeout = None;
    let client = kube::Client::try_from(config)?;
    if args.cleanup {
        info!(
            "Cleaning up stale pods (older than {} minutes)",
            args.cleanup_min
        );
        let pods: Api<Pod> = Api::all(client.clone());
        let mut pods_list = pods
            .list_metadata(&ListParams::default().labels(LABEL_KEY))
            .await?
            .items;
        let now = chrono::Utc::now();
        let limit = chrono::Duration::minutes(args.cleanup_min as i64);
        pods_list.retain(|pod| {
            pod.metadata
                .creation_timestamp
                .as_ref()
                .map_or(false, |t| now - t.0 > limit)
                || pod.metadata.labels.as_ref().map_or(false, |labels| {
                    labels.get(LABEL_KEY).map(|l| l.as_str()) == Some(LABEL_DELETE)
                })
        });
        info!("Found {} pods to delete", pods_list.len());
        for p in pods_list {
            let api: Api<Pod> = Api::namespaced(client.clone(), &p.metadata.namespace.unwrap());
            let name = p.metadata.name.unwrap();
            api.delete(&name, &Default::default()).await?;
            if !args.nowait {
                await_condition(api.clone(), &name, is_deleted(&p.metadata.uid.unwrap())).await?;
            }
        }
        info!("Done");
        return Ok(());
    }

    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), &args.namespace);

    let pvcs_list = pvcs.list(&Default::default()).await?;
    if let Some(name) = args.name {
        let read_only = Some(!args.rw);
        if args.rw {
            warn!("Volume will be mounted in read/write mode");
        }
        anyhow::ensure!(
            pvcs_list.into_iter().any(|pvc| pvc
                .metadata()
                .name
                .as_ref()
                .map_or(false, |n| n == &name)),
            "PVC {} not found",
            name
        );

        info!("Generating keys");
        let key = ssh_key::PrivateKey::random(
            &mut rand_core::OsRng,
            ssh_key::Algorithm::new("ssh-ed25519")?,
        )?;
        let key_file = tempfile::NamedTempFile::new()?;
        key.write_openssh_file(key_file.path(), ssh_key::LineEnding::default())?;

        info!("Creating pod");
        let yaml = include_str!("../templates/ssh.yaml");

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let mut pod: Pod = serde_yaml::from_str(yaml)?;
        pod.metadata = ObjectMeta {
            generate_name: Some(format!("pvc-inspect-{}-", name)),
            namespace: Some(args.namespace.clone()),
            labels: Some([(LABEL_KEY.into(), "1".into())].into()),
            ..Default::default()
        };
        let spec = pod.spec.get_or_insert(Default::default());

        let volumes = spec.volumes.get_or_insert(Default::default());
        volumes.push(Volume {
            name: "data".into(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: name,
                read_only,
            }),
            ..Default::default()
        });

        for container in &mut spec.containers {
            let env = container.env.get_or_insert(Default::default());
            env.push(EnvVar {
                name: "PUBLIC_KEY".into(),
                value: Some(key.public_key().to_openssh()?),
                ..Default::default()
            });
            let mounts = container.volume_mounts.get_or_insert(Default::default());
            mounts.push(VolumeMount {
                mount_path: "/data".into(),
                name: "data".into(),
                read_only,
                ..Default::default()
            });
        }
        let pods: Api<Pod> = Api::namespaced(client, &args.namespace);
        let pod = pods.create(&Default::default(), &pod).await?;

        let pod_name = pod.metadata.name.clone().unwrap();
        info!("Waiting for pod {:?}", pod_name);
        struct PodReady {}
        impl Condition<Pod> for PodReady {
            fn matches_object(&self, pod: Option<&Pod>) -> bool {
                let Some(status) = pod.and_then(|pod| pod.status.as_ref()) else {
                    return false;
                };
                status
                    .phase
                    .as_ref()
                    .map_or(false, |phase| phase == "Running")
                    && status
                        .container_statuses
                        .iter()
                        .flatten()
                        .map(|cs| cs.ready)
                        .all(std::convert::identity)
            }
        }

        await_condition(pods.clone(), &pod_name, PodReady {}).await?;
        std::thread::sleep(std::time::Duration::from_secs(1));

        info!("Pod created");
        info!("Starting port forwarding on port {}", port);
        // TODO: We could do this with Kube directly
        let mut forward = std::process::Command::new("kubectl")
            .args([
                "-n",
                &args.namespace,
                "port-forward",
                &pod_name,
                &format!("{}:2222", port),
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let stdout = forward.stdout.take().unwrap();
        let mut stdout = std::io::BufReader::new(stdout);
        let mut line = String::new();
        stdout.read_line(&mut line)?;

        let mount = if let Some(mountpoint) = args.mountpoint {
            info!("Mounting on {:?}", mountpoint);
            std::fs::create_dir_all(&mountpoint)?;
            let child = std::process::Command::new("sshfs")
                .args([
                    "ssh@127.0.0.1:/data",
                    "-o",
                    "auto_unmount",
                    "-o",
                    "UserKnownHostsFile=/dev/null",
                    "-o",
                    &format!("IdentityFile={}", key_file.path().to_str().unwrap()),
                    "-o",
                    "StrictHostKeyChecking=no",
                    "-f",
                    "-p",
                    &port.to_string(),
                    mountpoint.to_str().unwrap(),
                ])
                .stderr(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .spawn();
            match child {
                Ok(child) => Some(child),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    anyhow::bail!("`sshfs` not found in PATH.")
                }
                Err(e) => {
                    return Err(e).context("Failed to mount via SSHFS");
                }
            }
        } else {
            None
        };

        info!("Connecting to pod. Type Control+D to exit the shell");
        // As in kube/examples/pod_shell_crossterm.rs
        let mut exec = pods
            .exec(
                &pod_name,
                ["/bin/bash", "-c", "cd /data && /bin/bash"],
                &AttachParams::interactive_tty(),
            )
            .await?;
        crossterm::terminal::enable_raw_mode()?;
        let mut stdin = tokio_util::io::ReaderStream::new(tokio::io::stdin());
        let mut stdout = tokio::io::stdout();
        let mut output = tokio_util::io::ReaderStream::new(exec.stdout().unwrap());
        let mut input = exec.stdin().unwrap();
        loop {
            tokio::select! {
                message = stdin.next() => {
                    match message {
                        Some(Ok(message)) => {
                            let _ = input.write(&message).await?;
                        }
                        _ => {
                            break;
                        },
                    }
                },
                message = output.next() => {
                    match message {
                        Some(Ok(message)) => {
                            let _ = stdout.write(&message).await?;
                            stdout.flush().await?;
                        },
                        _ => {
                            break
                        },
                    }
                },
            };
        }
        crossterm::terminal::disable_raw_mode()?;

        // Cleanup

        if let Some(mut mount) = mount {
            info!("Unmounting");
            mount.kill()?;
        }
        info!("Stopping port forwarding");
        forward.kill()?;
        info!("Deleting pod");
        // Edit the label to mark the pod for deletion, to cover the use case where the user might
        // not have the right to delete pods
        let metadata = ObjectMeta {
            labels: Some([(LABEL_KEY.into(), LABEL_DELETE.into())].into()),
            ..Default::default()
        }
        .into_request_partial::<Pod>();
        pods.patch_metadata(
            &pod_name,
            &kube::api::PatchParams::apply("pv_inspect").force(),
            &kube::api::Patch::Apply(metadata),
        )
        .await?;
        pods.delete(&pod_name, &Default::default()).await?;
        if !args.nowait {
            info!("Waiting for deletion");
            await_condition(
                pods.clone(),
                &pod_name,
                is_deleted(&pod.metadata.uid.unwrap()),
            )
            .await?;
            info!("Pod deleted");
        }
    } else {
        info!("No PVC name provided, listing...");
        let table = pvcs_list.into_iter().map(|a| {
            let meta = a.metadata();
            Pvc {
                name: meta.name.clone().unwrap_or_default(),
                creation: meta
                    .creation_timestamp
                    .as_ref()
                    .map(|t| t.0.to_string())
                    .unwrap_or_default(),
                size: a
                    .spec
                    .and_then(|s| s.resources)
                    .and_then(|r| r.requests)
                    .and_then(|r| r.get("storage").cloned())
                    .map(|s| s.0)
                    .unwrap_or_default(),
            }
        });
        info!(
            "Volume claims in namespace {}:\n{}",
            args.namespace,
            tabled::Table::new(table).with(tabled::settings::style::Style::markdown())
        );
        warn!("Provide the name of the volume claim to inspect.")
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = main_impl().await {
        error!("{}", e);
        std::process::exit(2);
    }
    // Necessary because we intercepted the signal
    std::process::exit(0);
}

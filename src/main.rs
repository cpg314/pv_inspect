use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;
use k8s_openapi::api::core::v1::{
    PersistentVolumeClaim, PersistentVolumeClaimVolumeSource, Pod, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::Metadata;
use kube::api::{Api, AttachParams};
use kube::runtime::conditions::is_deleted;
use kube::runtime::wait::{await_condition, Condition};
use log::*;

/// Mount a PVC on a new pod, shell into it, and port-forward if desired.
#[derive(Parser)]
struct Flags {
    #[clap(long, short, default_value = "default")]
    namespace: String,
    /// Name of the PVC to inspect
    name: Option<String>,
    #[clap(long, value_enum, default_value_t=Template::Miniserve)]
    template: Template,
    /// Alternatively, path to a custom pod template
    #[clap(long, conflicts_with = "template")]
    template_yaml: Option<PathBuf>,
    /// Bind a port on the pod. Format: host:pod
    #[clap(long)]
    port: Option<PortBind>,
}

#[derive(Clone)]
struct PortBind {
    host: u16,
    pod: u16,
}
impl std::fmt::Display for PortBind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.pod)
    }
}

impl FromStr for PortBind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(s) = u16::from_str(s) {
            Ok(Self { host: s, pod: s })
        } else {
            let (host, pod) = s
                .split_once(',')
                .ok_or_else(|| anyhow::anyhow!("Invalid port binding"))?;
            Ok(Self {
                host: u16::from_str(host)?,
                pod: u16::from_str(pod)?,
            })
        }
    }
}

#[derive(Clone, PartialEq, clap::ValueEnum)]
enum Template {
    /// Shell and miniserve port-forwarded on 8080:8080
    Miniserve,
    /// A pod that sleeps
    Sleep,
    /// Shell and Samba mount port-forwarded on 8080:445
    Samba,
}

#[derive(tabled::Tabled)]
struct Pvc {
    name: String,
    creation: String,
    size: String,
}

async fn main_impl() -> anyhow::Result<()> {
    let mut args = Flags::parse();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut config = kube::Config::infer().await?;
    config.read_timeout = None;
    config.write_timeout = None;
    let client = kube::Client::try_from(config)?;
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), &args.namespace);

    let pvcs_list = pvcs.list(&Default::default()).await?;
    if let Some(name) = args.name {
        anyhow::ensure!(
            pvcs_list.into_iter().any(|pvc| pvc
                .metadata()
                .name
                .as_ref()
                .map_or(false, |n| n == &name)),
            "PVC {} not found",
            name
        );
        info!("Creating pod");
        let yaml = match (&args.template_yaml, &args.template) {
            (Some(path), _) => {
                info!("Using template at {:?}", path);
                std::fs::read_to_string(path)?
            }
            (None, Template::Miniserve) => {
                args.port = Some(PortBind {
                    host: 8080,
                    pod: 8080,
                });
                include_str!("../templates/miniserve.yaml").into()
            }
            (None, Template::Sleep) => include_str!("../templates/sleep.yaml").into(),
            (None, Template::Samba) => {
                args.port = Some(PortBind {
                    host: 8080,
                    pod: 445,
                });
                include_str!("../templates/samba.yaml").into()
            }
        };
        let mut pod: Pod = serde_yaml::from_str(&yaml)?;
        pod.metadata = ObjectMeta {
            generate_name: Some(format!("pvc-inspect-{}-", name)),
            namespace: Some(args.namespace.clone()),
            labels: Some([("pv-inspect".into(), "1".into())].into()),
            ..Default::default()
        };
        let spec = pod.spec.get_or_insert(Default::default());
        let volumes = spec.volumes.get_or_insert(Default::default());
        volumes.push(Volume {
            name: "data".into(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: name,
                read_only: Some(true),
            }),
            ..Default::default()
        });
        for container in &mut spec.containers {
            let mounts = container.volume_mounts.get_or_insert(Default::default());
            mounts.push(VolumeMount {
                mount_path: "/data".into(),
                name: "data".into(),
                read_only: Some(true),
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

        info!("Pod created");
        let forward = if let Some(port) = args.port {
            info!("Starting port forwarding on port {}", port);
            Some(
                // TODO: We could do this with Kube directly
                std::process::Command::new("kubectl")
                    .args([
                        "-n",
                        &args.namespace,
                        "port-forward",
                        &pod_name,
                        &port.to_string(),
                    ])
                    .stdout(std::process::Stdio::null())
                    .spawn()?,
            )
        } else {
            None
        };

        if args.template_yaml.is_none() {
            match args.template {
                Template::Miniserve => {
                    info!("Miniserve on http://localhost:8080");
                }
                Template::Samba => {
                    info!(
                        "Mount samba share with
sudo mount -t cifs //127.0.0.1/public samba -o port=8080 -o password=\"\""
                    );
                }
                _ => {}
            }
        }
        info!("Connecting to pod. Type Control+D to exit the shell");
        // As in kube/examples/pod_shell.rs
        let mut exec = pods
            .exec(
                &pod_name,
                ["/bin/bash", "-c", "cd /data && /bin/bash"],
                &AttachParams::interactive_tty(),
            )
            .await?;
        let mut stdin_writer = exec.stdin().unwrap();
        let mut stdout_reader = exec.stdout().unwrap();
        let mut stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        tokio::spawn(async move {
            tokio::io::copy(&mut stdin, &mut stdin_writer)
                .await
                .unwrap();
        });
        tokio::spawn(async move {
            tokio::io::copy(&mut stdout_reader, &mut stdout)
                .await
                .unwrap();
        });
        exec.join().await?;
        println!();

        // Cleanup
        if let Some(mut forward) = forward {
            info!("Stopping port forwarding");
            forward.kill()?;
        }
        info!("Deleting pod");
        info!("Waiting for deletion");
        pods.delete(&pod_name, &Default::default()).await?;
        await_condition(
            pods.clone(),
            &pod_name,
            is_deleted(&pod.metadata.uid.unwrap()),
        )
        .await?;
        info!("Pod deleted");
    } else {
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
}

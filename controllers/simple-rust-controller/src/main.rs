use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use kube::{
    api::{ListParams, PostParams},
    Api, Client, CustomResource,
};
use kube_runtime::controller::{Action, Context, Controller};

use chrono::{Local, Utc};
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::env;
use std::mem;
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

const KUBESECRET: &str = "varsecret";

#[cfg(target_arch = "wasm32")]
use {futures::task::SpawnExt, std::ops::Deref};

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("Kube error: {}", source))]
    #[snafu(context(false))]
    UnknownKubeError { source: kube::Error },
}

#[derive(CustomResource, Debug, Serialize, Deserialize, Default, Clone, JsonSchema)]
#[kube(
    kind = "TestResource",
    group = "amurant.io",
    version = "v1",
    namespaced
)]
pub struct TestResourceSpec {
    nonce: i64,
    updated_at: Option<MicroTime>,
}

/// The controller triggers this on reconcile errors
fn error_policy(_error: &Error, _ctx: Context<Data>) -> Action {
    Action::requeue(Duration::from_secs(1))
}

// Data we want access to in error/reconcile calls
struct Data {
    client: Client,
    //out_namespace: String,
    huge_mem_alloc: Arc<Vec<u8>>,
}

#[cfg(target_arch = "wasm32")]
fn main() {
    let exec = kube_runtime_abi::get_mut_executor();
    // Start the main
    exec.deref()
        .borrow_mut()
        .spawner()
        .spawn(main_async())
        .unwrap();
    // Give a little push to the executor
    exec.deref().borrow_mut().run_until_stalled();
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    main_async().await;
}

async fn main_async() {
    tracing_subscriber::fmt::init();
    println!("main launched simple child");

    let client = Client::try_default()
        .await
        .expect("could not create kube client");

    let in_resources: Api<TestResource> = Api::namespaced(client.clone(), "default");

    let heap_mem_size = env::var("HEAP_MEM_SIZE").unwrap_or("0".to_string());
    let heap_mem_size = heap_mem_size.parse::<usize>().unwrap();

    //make big memory
    let mut huge_mem_alloc = Vec::with_capacity(heap_mem_size);
    for i in 0..heap_mem_size {
        huge_mem_alloc.push((i + 9) as u8);
    }

    println!("size of  vector  is {}", mem::size_of_val(&*huge_mem_alloc));
    let huge_mem_alloc = Arc::new(huge_mem_alloc);

    Controller::new(
        in_resources,
        ListParams {
            bookmarks: false,
            // TODO IMPORTANT timeout parameter is currently set to 290 seconds for every watch then it starts a new watch this is pretty random and messes with the accuracy of the prediction models, and should be increased to probably infinite however there are some bugs in Kube.rs that say you should not go above 295 seconds, should be addressed...
            ..ListParams::default()
        },
    )
    .run(
        reconcile,
        error_policy,
        Context::new(Data {
            client,
            // out_namespace,
            huge_mem_alloc,
        }),
    )
    .for_each(|res| async move {
        match res {
            Ok((obj, _)) => debug!("Reconciled {:?}", obj),
            Err(e) => debug!("Reconcile error: {:?}", e),
        }
    })
    .await;
}

async fn reconcile(resource: Arc<TestResource>, ctx: Context<Data>) -> Result<Action, Error> {
    let now_timestamp = MicroTime(Local::now().with_timezone(&Utc));

    println!("{:?}  reconcile called", now_timestamp.0.to_string());

    let client = ctx.get_ref().client.clone();
    //or use provided resource in arc
    let resource: Api<TestResource> = Api::namespaced(client, "default");

    let now_timestamp = MicroTime(Local::now().with_timezone(&Utc));

    match resource.get(KUBESECRET).await {
        Ok(existing) => {
            let size = ctx.get_ref().huge_mem_alloc.len();
            println!("{:?}    child node reconcile changed secret on {:?} to {:?}  with  {:?} byte buffer", now_timestamp.0.to_string(),existing.spec.nonce -1,existing.spec.nonce,size );
        }
        Err(kube::Error::Api(ae)) if ae.code == 404 => {
            println!("No testresource, made new one");
            let index: i64 = 1;
            resource
                .create(
                    &PostParams::default(),
                    &test_resource(KUBESECRET, &index, now_timestamp),
                )
                .await?;
        }

        Err(e) => println!("getting resource {:?}", e),
    }

    Ok(Action::await_change())
}

fn test_resource(name: &str, nonce: &i64, start_timestamp: MicroTime) -> TestResource {
    TestResource {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: TestResourceSpec {
            nonce: *nonce,
            updated_at: Some(start_timestamp),
        },
    }
}

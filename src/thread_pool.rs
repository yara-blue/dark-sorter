// use color_eyre::eyre::Context;
// use std::thread::{JoinHandle, available_parallelism};
// use thread_priority::{ThreadBuilder, ThreadPriority};
//
// /// Thread pool that runs the blocking work at a lower priority then the rest of
// /// the program
// struct BackgroundPool {
//     threads: Vec<JoinHandle<()>>,
// }
//
// impl BackgroundPool {
//     fn new() -> color_eyre::Result<Self> {
//         Ok(Self {
//             threads: (0..available_parallelism()
//                 .wrap_err("Could not get available parallelism")?)
//                 .into_iter()
//                 .map(|i| {
//                     ThreadBuilder::default()
//                         .name(format!("BackgroundPool thread {i}"))
//                         .priority(ThreadPriority::Min)
//                         .stack_size(10)
//                 }),
//         })
//     }
// }

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|argument| argument == "--media-worker")
    {
        if let Err(error) = frameflow_media::run_media_worker_stdio() {
            eprintln!("media worker failed: {error}");
            std::process::exit(1);
        }
    } else if arguments
        .iter()
        .any(|argument| argument == "--capture-worker")
    {
        let command_pipe_name = arguments
            .windows(2)
            .find(|pair| pair[0] == "--ipc-command-pipe")
            .and_then(|pair| pair[1].to_str());
        let response_pipe_name = arguments
            .windows(2)
            .find(|pair| pair[0] == "--ipc-response-pipe")
            .and_then(|pair| pair[1].to_str());
        let result = match (command_pipe_name, response_pipe_name) {
            (Some(command_pipe_name), Some(response_pipe_name)) => {
                frameflow_recorder::run_capture_worker_named_pipes(
                    command_pipe_name,
                    response_pipe_name,
                )
            }
            (None, None) => frameflow_recorder::run_capture_worker_stdio(),
            _ => Err("capture worker requires both IPC pipe names".to_owned()),
        };
        if let Err(error) = result {
            eprintln!("capture worker failed: {error}");
            std::process::exit(1);
        }
    } else {
        frameflow_lib::run();
    }
}

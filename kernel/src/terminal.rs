use kernel_userspace::{
    fs::{read_full_file, stat_file, ReadResponse, StatResponse, FS_READ_FULL_FILE, FS_STAT},
    service::{
        generate_tracking_number, get_public_service_id, send_and_get_response_sync, SpawnProcess,
        SID,
    },
    syscall::exit,
};

pub fn load_terminal() {
    let fs_sid = get_public_service_id("FS").unwrap();
    let file = stat_file(fs_sid, 0, "terminal.elf");

    if file.get_message_header().data_type != FS_STAT {
        panic!("STAT FAILED")
    };
    let stat: StatResponse = file.get_data_as().unwrap();

    let file = match stat {
        StatResponse::File(f) => f,
        StatResponse::Folder(_) => {
            panic!("Not a file");
        }
    };

    let contents = read_full_file(fs_sid, 0, file.node_id);

    if contents.get_message_header().data_type != FS_READ_FULL_FILE {
        panic!("Error reading file");
    }

    let contents_buffer = contents.get_data_as::<ReadResponse>().unwrap();

    send_and_get_response_sync(
        SID(1),
        kernel_userspace::service::MessageType::Request,
        generate_tracking_number(),
        1,
        SpawnProcess {
            elf: contents_buffer.data,
            args: &[],
        },
        0,
    );

    exit()
}

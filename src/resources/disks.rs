#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct Disk {
    pub mountpoint: String,
    pub fs: String,
    pub total_space: u64,
    pub available_space: u64,
    pub is_removable: bool,
    pub is_read_only: bool,
}

pub fn disk_list() -> Vec<Disk> {
    use sysinfo::Disks;

    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .into_iter()
        .map(|d| Disk {
            mountpoint: d.mount_point().to_string_lossy().to_string(),
            fs: d.file_system().to_string_lossy().to_string(),
            total_space: d.total_space(),
            available_space: d.available_space(),
            is_read_only: d.is_read_only(),
            is_removable: d.is_removable(),
        })
        .collect()
}

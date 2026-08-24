use std::collections::{HashMap, HashSet};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct LsblkDevice {
    pub name: String,
    #[serde(rename = "type")]
    pub device_type: String,
    pub children: Option<Vec<LsblkDevice>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LsblkOutput {
    pub blockdevices: Option<Vec<LsblkDevice>>,
}

fn build_mpath_map(devices: &[LsblkDevice], map: &mut HashMap<String, HashSet<String>>) {
    let mut stack = vec![(devices, None)];

    while let Some((devs, current_mpath)) = stack.pop() {
        for dev in devs {
            let next_mpath = if dev.device_type == "mpath" {
                Some(dev.name.as_str())
            } else {
                current_mpath
            };

            if let Some(mpath) = next_mpath {
                map.entry(dev.name.clone())
                    .or_default()
                    .insert(mpath.to_string());
            }

            if let Some(children) = &dev.children {
                stack.push((children, next_mpath));
            }
        }
    }
}

fn main() {
    let content = std::fs::read_to_string("test-data/pvesh/lsblk.json").unwrap();
    let out = serde_json::from_str::<LsblkOutput>(&content).unwrap();
    let mut map = HashMap::new();
    build_mpath_map(&out.blockdevices.unwrap(), &mut map);
    println!("Does map have mpatha? {:?}", map.get("mpatha"));

    // Also test extract disks logic
    let mut matched_mpaths = None;
    let dm_name = "/dev/mapper/mpatha";
    let dm_name_stripped = dm_name.strip_prefix("/dev/mapper/").unwrap();
    if let Some(mpaths) = map.get(dm_name_stripped) {
        matched_mpaths = Some(mpaths);
    }
    println!("Matched mpaths: {:?}", matched_mpaths);
}

use std::{error::Error, fs};

type ResultDyn<T> = Result<T, Box<dyn Error>>;

const KNOWN_SHELLS: &[&str] = &[
    "ash",    //
    "bash",   //
    "elvish", //
    "fish",   //
    "nu",     //
    "pwsh",   //
    "tcsh",   //
    "zsh",    //
];

fn get_process_status_field(pid: u32, field: &str) -> ResultDyn<String> {
    let status_bytes =
        fs::read(format!("/proc/{pid:?}/status")).map_err(|_| format!("no such pid: {pid:?}"))?;
    let status_str = String::from_utf8(status_bytes)?;
    let status_str = status_str
        .split('\n')
        .find(|&x| x.starts_with(field))
        .ok_or_else(|| format!("error parsing /proc/{pid:?}/status"))?;
    let field_contents = status_str
        .strip_prefix(&format!("{field}:"))
        .ok_or_else(|| "bad parsing".to_string())?
        .trim()
        .to_owned();
    Ok(field_contents)
}

fn get_parent_pid(pid: u32) -> ResultDyn<u32> {
    Ok(get_process_status_field(pid, "PPid")?.parse::<u32>()?)
}

fn get_process_name(pid: u32) -> ResultDyn<String> {
    get_process_status_field(pid, "Name")
}

fn get_all_parents_pid(pid: u32) -> ResultDyn<Vec<u32>> {
    let mut res = Vec::<u32>::new();
    let mut pid = pid;
    loop {
        match get_parent_pid(pid) {
            Ok(parent_id) if parent_id != 0 => {
                res.push(parent_id);
                pid = parent_id;
            }
            Ok(_) => return Ok(res),
            Err(e) => return Err(e),
        }
    }
}

pub fn select_shell_from_pid(pid: u32) -> ResultDyn<String> {
    let parents = get_all_parents_pid(pid)?;
    let parents_names: Result<Vec<_>, _> =
        parents.iter().map(|&pid| get_process_name(pid)).collect();
    let shell = parents_names?
        .into_iter()
        .find(|x| KNOWN_SHELLS.contains(&x.as_str()));
    shell.ok_or("no shell found".into())
}

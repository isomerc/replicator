//! Dev probe: dump the structure of a settings file's groups, with
//! values truncated, to learn the real schema. Not shipped.
//!
//!   cargo run --example dump_settings -- <file.dat> [group]

use blue_marshal::{decode, Value};

fn brief(v: &Value, depth: usize, budget: &mut usize) -> String {
    if *budget == 0 {
        return "...".into();
    }
    *budget -= 1;
    let pad = "  ".repeat(depth);
    match v {
        Value::Dict(items) => {
            let mut s = format!("Dict({})", items.len());
            if depth < 4 {
                for (k, val) in items.iter().take(12) {
                    s.push_str(&format!(
                        "\n{pad}  {:?} => {}",
                        short_key(k),
                        brief(val, depth + 1, budget)
                    ));
                }
                if items.len() > 12 {
                    s.push_str(&format!("\n{pad}  ... {} more", items.len() - 12));
                }
            }
            s
        }
        Value::List(items) | Value::Tuple(items) => {
            let name = if matches!(v, Value::List(_)) {
                "List"
            } else {
                "Tuple"
            };
            let mut s = format!("{name}({})", items.len());
            if depth < 4 {
                for val in items.iter().take(8) {
                    s.push_str(&format!("\n{pad}  - {}", brief(val, depth + 1, budget)));
                }
                if items.len() > 8 {
                    s.push_str(&format!("\n{pad}  ... {} more", items.len() - 8));
                }
            }
            s
        }
        Value::Str(s) => format!("Str({:?})", s.chars().take(40).collect::<String>()),
        Value::Bytes(b) => format!("Bytes({} bytes)", b.len()),
        other => format!("{other:?}"),
    }
}

fn short_key(k: &Value) -> String {
    match k {
        Value::Str(s) => s.chars().take(40).collect(),
        Value::Bytes(b) => String::from_utf8_lossy(b).chars().take(40).collect(),
        other => format!("{other:?}"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: dump_settings <file.dat> [group] [inner]");
    let want = args.next();
    let inner = args.next();
    let bytes = std::fs::read(&path).expect("read file");
    let d = decode(&bytes).expect("decode");
    let Value::Dict(items) = d.value else {
        panic!("top level is not a dict");
    };
    for (k, v) in &items {
        let name = short_key(k);
        match &want {
            Some(w) if &name != w => continue,
            _ => {}
        }
        // Drill: group -> (timestamp, dict) -> inner key, dumped deep.
        if let (Some(_), Some(inner_key)) = (&want, &inner) {
            let Value::Dict(entries) = v else { continue };
            for (ik, iv) in entries {
                if &short_key(ik) != inner_key {
                    continue;
                }
                let payload = match iv {
                    Value::Tuple(t) if t.len() == 2 => &t[1],
                    other => other,
                };
                if let Value::Dict(rows) = payload {
                    for (rk, rv) in rows {
                        let mut budget = 60usize;
                        println!("{} = {}", short_key(rk), brief(rv, 1, &mut budget));
                    }
                } else {
                    let mut budget = 200usize;
                    println!("{}", brief(payload, 0, &mut budget));
                }
            }
            continue;
        }
        let mut budget = 400usize;
        println!("group {name}: {}", brief(v, 1, &mut budget));
    }
}

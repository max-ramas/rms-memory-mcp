with open("src/install.rs", "r") as f:
    content = f.read()

target = """            }
        }
    }
    
    println!("[✅] Installation sweep completed.");"""

replacement = """            }
        } else {
            eprintln!("[⚠️] Failed to safely patch {}. It might be malformed or use an unsupported format.", candidate.display());
        }
    }
    
    println!("[✅] Installation sweep completed.");"""

if target in content:
    content = content.replace(target, replacement)
    with open("src/install.rs", "w") as f:
        f.write(content)
    print("Patched error branch")
else:
    print("Target not found")

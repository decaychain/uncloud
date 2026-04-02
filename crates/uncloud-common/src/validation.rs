pub fn validate_username(username: &str) -> Result<(), &'static str> {
    if username.len() < 3 {
        return Err("Username must be at least 3 characters");
    }
    if username.len() > 32 {
        return Err("Username must be at most 32 characters");
    }
    if !username
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err("Username can only contain letters, numbers, underscores, and hyphens");
    }
    Ok(())
}

pub fn validate_email(email: &str) -> Result<(), &'static str> {
    if !email.contains('@') {
        return Err("Invalid email address");
    }
    if email.len() > 255 {
        return Err("Email too long");
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if password.len() < 8 {
        return Err("Password must be at least 8 characters");
    }
    if password.len() > 128 {
        return Err("Password too long");
    }
    Ok(())
}

pub fn validate_filename(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("Filename cannot be empty");
    }
    if name.len() > 255 {
        return Err("Filename too long");
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("Filename contains invalid characters");
    }
    if name == "." || name == ".." {
        return Err("Invalid filename");
    }
    Ok(())
}

pub fn validate_folder_name(name: &str) -> Result<(), &'static str> {
    validate_filename(name)
}

pub fn format_bytes(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;
    const TB: i64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

use chrono::{DateTime, TimeZone, Utc};

/// Extensions to the chrono DateTime type for HTTP.
pub trait DateTimeHttp {
    /// Returns an HTTP format date string, such as `Sat, 26 Oct 1985 01:22:00 GMT`.
    fn to_http_date(&self) -> String;
}

impl<Tz: TimeZone> DateTimeHttp for DateTime<Tz> {
    fn to_http_date(&self) -> String {
        format!(
            "{} GMT",
            self.with_timezone(&Utc).format("%a, %e %b %Y %H:%M:%S")
        )
    }
}

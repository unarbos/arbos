    static func dayLabel(_ date: Date, today: Date = Date(), calendar: Calendar = .current) -> String {
        if calendar.isDate(date, inSameDayAs: today) { return "Today" }
        if let y = calendar.date(byAdding: .day, value: -1, to: today), calendar.isDate(date, inSameDayAs: y) {
            return "Yesterday"
        }
        let start = calendar.startOfDay(for: date)
        let todayStart = calendar.startOfDay(for: today)
        let days = calendar.dateComponents([.day], from: start, to: todayStart).day ?? 0
        let f = DateFormatter()
        f.calendar = calendar
        f.locale = Locale(identifier: "en_US_POSIX")
        if (0..<7).contains(days) {
            f.dateFormat = "EEEE"
        } else if calendar.component(.year, from: date) == calendar.component(.year, from: today) {
            f.dateFormat = "MMM d"
        } else {
            f.dateFormat = "MMM d, yyyy"
        }
        return f.string(from: date)
    }

    static func stamp(_ atMs: Int64, now: Date = Date(), calendar: Calendar = .current) -> String {
        let date = Date(timeIntervalSince1970: Double(atMs) / 1000)
        let f = DateFormatter()
        f.calendar = calendar
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "h:mm a"
        let time = f.string(from: date)
        if calendar.isDate(date, inSameDayAs: now) { return "Today \(time)" }
        if let y = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(date, inSameDayAs: y) {
            return "Yesterday \(time)"
        }
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day ?? 0
        if (0..<7).contains(days) {
            f.dateFormat = "EEE"
            return "\(f.string(from: date)) \(time)"
        }
        f.dateFormat = calendar.component(.year, from: date) == calendar.component(.year, from: now) ? "MMM d" : "MMM d, yyyy"
        return "\(f.string(from: date)), \(time)"
    }

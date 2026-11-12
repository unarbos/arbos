// FFI parser/math collaborators are outside this link-card fixture and must never be called.
enum MarkdownBlock { case paragraph(UInt8,String), heading(UInt8,UInt8,String), bullets(UInt8,[String]), numbered(UInt8,[String]), code(UInt8,String,String), table(UInt8,[String],[[String]]), math(UInt8,String) }
func markdownBlocks(text: String) -> [MarkdownBlock] { fatalError("unexpected markdown parse in link-card fixture") }
enum MathText { static func plain(_ text:String)->String { fatalError("unexpected math in link-card fixture") } }
// Date-divider FFI is independently tested by the project; fixtures use no divider.
func showsTimeDivider(previousMs:Int64,atMs:Int64,utcOffsetSeconds:Int32)->Bool { false }

enum Markdown {static func polishAssistantProse(_ text:String)->String{fatalError("markdown out of scope")};static func blocks(_ text:String)->[Block]{fatalError("markdown out of scope")};static func isJSON(_ block:Block)->Bool{fatalError("markdown out of scope")};static func plain(_ text:String)->String{fatalError("markdown out of scope")}}
enum OptimisticNotice {static func joinNames(_ names:[String])->String{fatalError("membership out of scope")}}
enum GroupActivity {static func isAuditLine(_ text:String)->Bool{fatalError("system lines out of scope")}}
func mentionSlug(_ text:String)->String{fatalError("project naming out of scope")}

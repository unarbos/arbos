struct FixtureUser { var id:String; var name:String?; var login:String }
struct FixtureProject { var name:String }
struct FixtureMember { var id:String;var member:String;var project:FixtureProject?;var name:String?;var handle:String }
struct FixturePeople { var agents:[FixtureMember]=[];var humans:[FixtureMember]=[];var isGroup=false }
final class FixtureModel { var messagesVersion:[String:Int]=[:];var tailEditVersion:[String:Int]=[:];var user:FixtureUser?;var channelMe:String?;var myUserId:String?;var linkPreviews=true;func hiddenSenders(in people:FixturePeople)->Set<String>{fatalError("people not in fixture")}}
enum PerfSignpost { enum RowsPath {case full,tail,append,unchanged};static var lastPath:RowsPath = .full; static func rowsBegin()->Int{0};static func rowsEnd(_ mark:Int,path:RowsPath,messages:Int,rows:Int,window:Int){lastPath=path}}
// Preview fixture is already admitted/current; this does not test account ownership.
struct FixtureCurrentAccount { func accepts(_ model: FixtureModel) -> Bool { true } }
final class CacheFixture {
 let account = FixtureCurrentAccount()
 let model=FixtureModel();var messages:[Message]=[];var isPeople=false;var isChannel=false;var people:FixturePeople?;var peopleSlot="people";var channelSlot="channel";var chatId="synthetic-chat";var projectId="synthetic-project"
 var refreshVersion:(chat:Id,version:Int?)?;var refreshIds:[Id]=[];var rows:[Row]=[];var messageById:[Id:Message]=[:];var rowsVersion=0;var windowSkipEnd=0;var windowCount=100;var refreshLinkPreviews:Bool?
 /* ACTUAL_REFRESH */
 func rebuild(){refreshRows()}
}

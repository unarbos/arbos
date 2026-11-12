let preview=LinkPreview(url:"https://synthetic.test/path",canonicalUrl:"https://synthetic.test/path",provider:"web",providerName:"Web",type:"page",title:"Synthetic preview")
func message(_ id:String,role:Role = .user,body:String = "https://synthetic.test/path")->Message {
 Message(id:id,chatId:"synthetic-chat",role:role,sender:role == .user ? "user:synthetic" : "agent:synthetic",status:.done,turn:nil,decision:nil,subagentId:nil,subagentName:nil,inReplyTo:nil,usage:[:],model:nil,createdAt:1_700_000_000_000,completedAt:nil,pending:false,parts:role == .user ? [.text(id:"text",text:body),.linkPreview(id:"preview",preview:preview)] : [.linkPreview(id:"preview",preview:preview)],hiddenCount:0)
}
func hasPreview(_ rows:[Row])->Bool { rows.contains { row in switch row.kind {case .user(_,_,_,let p),.other(_,_,_,_,_,let p):return p != nil;case .assistant(let blocks,_,_,_),.member(let blocks,_,_,_,_,_,_,_):return blocks.contains{if case .linkPreview = $0{return true};return false};default:return false}} }


func plainText(_ rows:[Row])->String {rows.compactMap{row in if case .user(let text,_,_,_) = row.kind{return text};return nil}.joined(separator:"|")}
func record(_ name:String,_ fields:[String:Any]){var value=fields;value["case"]=name;let data=try!JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]);print(String(data:data,encoding:.utf8)!)}
let ordinary=message("a")
record("default-user",["preview":hasPreview(defaultRows([ordinary]))])
record("off-user",["preview":hasPreview(direct(false,[ordinary])),"text":plainText(direct(false,[ordinary]))])
let assistant=message("agent",role:.assistant,body:"")
record("on-assistant",["preview":hasPreview(direct(true,[assistant]))])
record("off-assistant",["preview":hasPreview(direct(false,[assistant]))])
var removed=ordinary;removed.preview="off"
record("sender-removed",["preview":hasPreview(direct(true,[removed])),"text":plainText(direct(true,[removed]))])
let offFirst=bind(false,[ordinary]);let onSecond=bind(true,[ordinary]);record("off-caller-survives-on",["preview":hasPreview(offFirst()),"other":hasPreview(onSecond())])
let onFirst=bind(true,[ordinary]);let offSecond=bind(false,[ordinary]);record("on-caller-survives-off",["preview":hasPreview(onFirst()),"other":hasPreview(offSecond())])
func cache()->CacheFixture {let result=CacheFixture();setPolicy(result,true);result.messages=[message("a"),message("b"),message("c")];result.model.messagesVersion[result.chatId]=1;result.rebuild();return result}
let unchanged=cache();let oldFirst=unchanged.rows[0];unchanged.rebuild();record("unchanged-positive",["preview":hasPreview(unchanged.rows),"reused":unchanged.rows[0].sharesStorage(with:oldFirst),"path":String(describing:PerfSignpost.lastPath)])
let warm=cache();setPolicy(warm,false);warm.rebuild();record("policy-unchanged-messages",["preview":hasPreview(warm.rows),"text":plainText(warm.rows),"path":String(describing:PerfSignpost.lastPath)])
let append=cache();setPolicy(append,false);append.messages.append(message("d"));append.model.messagesVersion[append.chatId]=2;append.rebuild();record("policy-plus-append",["preview":hasPreview(append.rows),"path":String(describing:PerfSignpost.lastPath)])
let tail=cache();setPolicy(tail,false);tail.messages[2]=message("c",body:"changed tail");tail.model.messagesVersion[tail.chatId]=2;tail.model.tailEditVersion[tail.chatId]=2;tail.rebuild();record("policy-plus-tail",["preview":hasPreview(tail.rows),"path":String(describing:PerfSignpost.lastPath)])
let normalTail=cache();let beforeTail=normalTail.rows[0];normalTail.messages[2]=message("c",body:"changed tail");normalTail.model.messagesVersion[normalTail.chatId]=2;normalTail.model.tailEditVersion[normalTail.chatId]=2;normalTail.rebuild();record("same-policy-tail-positive",["preview":hasPreview(normalTail.rows),"reused":normalTail.rows[0].sharesStorage(with:beforeTail),"path":String(describing:PerfSignpost.lastPath)])

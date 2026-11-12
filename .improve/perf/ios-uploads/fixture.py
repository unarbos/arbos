"""Build only the selected actual send methods, with explicit test collaborators."""
from pathlib import Path
import hashlib
import re
import support
HERE = Path(__file__).resolve().parent
P = 'apps/ios/Arbos/Arbos/'


def context(tree, baseline=False, api_path=None):
    app_path = tree/(P+'Store/AppModel.swift')
    api_path = api_path or tree/(P+'Sync/RelayAPI.swift')
    app, api = app_path.read_text(), api_path.read_text()
    owned = 'struct AccountWork' in app
    sources = {P+'Store/AppModel.swift': app_path, P+'Sync/RelayAPI.swift': api_path}
    if owned:
        path = tree/(P+'Sync/WorkLifetime.swift')
        sources[P+'Sync/WorkLifetime.swift'] = path
        helper = path.read_text()
    else:
        helper = ''
    source = app
    if baseline:
        path = HERE/('serial-owned.swift' if owned else 'serial-main.swift')
        source = path.read_text()
        sources['.improve/perf/ios-uploads/'+path.name] = path
    hashes = {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in sources.items()}
    return {'app': app, 'api': api, 'selected': source, 'helper': helper, 'owned': owned,
            'sources': sources, 'hashes': hashes}


def declarations(ctx):
    text = ctx['helper']
    if ctx['owned']:
        at = ctx['app'].index('struct AccountWork {')
        text += ctx['app'][at:ctx['app'].index('\n}', at)+2]+'\n'
    return text


def model(ctx, capture_commit=False):
    text = support.M
    if not ctx['owned']:
        text = text.replace('var accountLifetime=WorkLifetime();', '')
        text = '\n'.join(line for line in text.splitlines()
                         if not line.strip().startswith(('func replace(', 'func refreshTokens(')))+'\n'
    if capture_commit:
        text = text.replace(' init(_ s:Session)', ' var onCommit:(()->Void)?\n init(_ s:Session)')
        text = text.replace('messageId:nil))}', 'messageId:nil));onCommit?()}')
        text = text.replace('messageId:messageId))}', 'messageId:messageId));onCommit?()}')
    if ctx['owned']:
        text += support.method(ctx['app'], 'accountWork')+'\n'+support.method(ctx['app'], 'boundCallAPI')+'\n'
    return text


def branches(ctx):
    source = ctx['selected']
    text = ''
    if 'func uploadAttachments(' in source:
        at = source.index('    private enum AttachmentUpload {')
        text += source[at:source.index('\n    }', at)+6]+'\n'+support.method(source, 'uploadAttachments')+'\n'
    text += support.method(source, 'send')+'\n'
    people = support.method(source, 'sendPeople')
    begin = people.index('        markUnacked(chatId: chatId, messageId: messageId)')
    if 'let work = accountWork()' in people[:begin]:
        begin = people.rindex('        guard let work = accountWork()', 0, begin)
    text += '''    func peopleUpload(chatId:String="chat",attachments:[PendingAttachment],forwardedFrom:String?="forward",replyRef:ReplyRef?=nil,extraParts:[[String:Any]]=[["kind":"chart"]]) {
        let doc=CoreChatDoc();let messageId="message";let text="synthetic";let author=CoreChatDoc.Author(id:"A");let mentions=["user:synthetic"];let reply:String?="reply"
'''+people[begin:]
    return text


def stable(ctx):
    return ctx['hashes'] == {name: hashlib.sha256(path.read_bytes()).hexdigest()
                             for name, path in ctx['sources'].items()}

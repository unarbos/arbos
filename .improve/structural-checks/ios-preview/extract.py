from pathlib import Path
import json,hashlib,argparse
BASE=Path(__file__).resolve().parent
def block(text, anchor, derive=False):
    if text.count(anchor) != 1:
        raise ValueError('Source anchor missing or ambiguous')
    start = text.index(anchor)
    if derive:
        start = text.rfind('#[derive(', 0, start)
        if start < 0:
            raise ValueError('Missing actual derive')
    opening = text.index('{', text.index(anchor))
    index, depth = opening, 0
    while index < len(text):
        if text.startswith('//', index):
            end = text.find('\n', index)
            index = len(text) if end < 0 else end + 1
            continue
        if text.startswith('/*', index):
            end = text.find('*/', index + 2)
            if end < 0:
                raise ValueError('Unterminated source comment')
            index = end + 2
            continue
        if text[index] == '"':
            index += 1
            while index < len(text):
                if text[index] == '\\':
                    index += 2
                elif text[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            continue
        if text[index] == '{':
            depth += 1
        elif text[index] == '}':
            depth -= 1
            if depth == 0:
                return text[start:index + 1]
        index += 1
    raise ValueError('Unbalanced selected source')


def binding(transcript):
 if 'static var linkPreviewsEnabled' in transcript:
  return 'func setPolicy(_ cache:CacheFixture,_ enabled:Bool){cache.model.linkPreviews=enabled;Transcript.linkPreviewsEnabled=enabled}\nfunc bind(_ enabled:Bool,_ messages:[Message])->()->[Row]{Transcript.linkPreviewsEnabled=enabled;return{Transcript.rows(messages,projectId:"synthetic-project")}}\nfunc direct(_ enabled:Bool,_ messages:[Message])->[Row]{Transcript.linkPreviewsEnabled=enabled;return Transcript.rows(messages,projectId:"synthetic-project")}\nfunc defaultRows(_ messages:[Message])->[Row]{Transcript.linkPreviewsEnabled=true;return Transcript.rows(messages,projectId:"synthetic-project")}\n'
 return 'func setPolicy(_ cache:CacheFixture,_ enabled:Bool){cache.model.linkPreviews=enabled}\nfunc bind(_ enabled:Bool,_ messages:[Message])->()->[Row]{return{Transcript.rows(messages,projectId:"synthetic-project",linkPreviews:enabled)}}\nfunc direct(_ enabled:Bool,_ messages:[Message])->[Row]{Transcript.rows(messages,projectId:"synthetic-project",linkPreviews:enabled)}\nfunc defaultRows(_ messages:[Message])->[Row]{Transcript.rows(messages,projectId:"synthetic-project")}\n'

def prepare(tree,output):
 model=(tree/'apps/ios/Arbos/Arbos/Model/Model.swift').read_text();transcript=(tree/'apps/ios/Arbos/Arbos/UI/Transcript.swift').read_text();media=(tree/'apps/ios/Arbos/Arbos/UI/MediaViewer.swift').read_text();chat=(tree/'apps/ios/Arbos/Arbos/UI/ChatView.swift').read_text();items={}
 def take(text,anchor):
  s=block(text,anchor);items[anchor]=hashlib.sha256(s.encode()).hexdigest();return s
 types=['enum Role:', 'enum MessageStatus:', 'enum TurnKind:', 'enum StatusKind:', 'struct ToolCall:', 'struct AttachmentSource:', 'struct Attachment:', 'struct ChoiceOption:', 'struct Question:', 'struct SecretOffer:', 'enum Part:', 'struct MessageReaction:', 'struct JobRef:', 'struct ReplyRef:', 'struct MembershipNotice:', 'struct MessageBody:', 'struct ChartBody:', 'struct Message:', 'struct LinkPreview:']
 result='import Foundation\nimport CoreGraphics\ntypealias Id=String\n\n'+'\n\n'.join(take(model,anchor)for anchor in types)+'\n\n'
 result+='\n\n'.join(take(media,anchor)for anchor in ['struct MediaItem:','enum MediaAlbum {'])+'\n\n'
 result+='\n\n'.join(take(transcript,anchor)for anchor in ['enum Block:','struct Quote:','enum RowKind:','enum MediaSender:','struct Row:','enum Transcript {','enum Delivery:'])+'\n\n'
 result+=(BASE/'collaborators.swift').read_text()+'\n'+(BASE/'cache_fixture.swift').read_text().replace('/* ACTUAL_REFRESH */',take(chat,'private func refreshRows()'))+'\n'+binding(transcript)+'\n'+(BASE/'driver.swift').read_text()
 output.write_text(result);output.with_suffix('.provenance.json').write_text(json.dumps({'selected_items':items,'source_method':'literal actual Swift row builder, DTOs, model methods, refreshRows; only test enclosure/observer collaborators added'},indent=2)+'\n')
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();prepare(a.root,a.output)

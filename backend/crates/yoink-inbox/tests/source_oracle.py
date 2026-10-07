"""Read-only preserved source oracle. Execute ONLY inside the existing image.
No network, service bootstrap, secrets, imports of plugin initializers or writes.
"""
import asyncio, importlib.util, json, sys
from pathlib import Path
import httpx
ROOT=Path('/source')
def load(name,path):
    spec=importlib.util.spec_from_file_location(name,ROOT/path)
    m=importlib.util.module_from_spec(spec);sys.modules[name]=m;spec.loader.exec_module(m);return m
schemas=load('inbox_oracle_schemas','plugins/yoink-inbox/src/yoink_inbox/api/schemas.py')
fetch=load('inbox_oracle_fetch','plugins/yoink-insight/src/yoink_insight/services/fetch.py')
repo={'full_name':'a/b','description':'Répo','stargazers_count':7,'forks_count':2,'open_issues_count':3,'language':None,'license':None,'default_branch':'main','html_url':'https://github.com/a/b'}
issue={'number':1,'title':'Title','state':'open','user':{'login':'Ada'},'body':None,'base':{'ref':'main'},'head':{'ref':'feature'}}
responses={
'repos/a/b':repo,'repos/a/b/readme':{'content':'UmVhZG1l'},
'repos/a/b/git/trees/HEAD?recursive=1':{'tree':[{'type':'tree','path':'src'},{'type':'blob','path':'src/a.rs'},{'type':'blob','path':'docs/a.md'}]},
'repos/a/b/git/trees/main?recursive=1':{'tree':[{'type':'tree','path':'src'},{'type':'blob','path':'src/a.rs'},{'type':'blob','path':'docs/a.md'}]},
'repos/a/b/contents/src/a.rs?ref=main':{'encoding':'base64','content':'Y29udGVudHM='},
'repos/a/b/contents/src/a.rs?ref=feature%2Fx':{'encoding':'base64','content':'Y29udGVudHM='},
'repos/a/b/issues/1':issue,'repos/a/b/pulls/1':issue,
'repos/a/b/issues/1/comments?per_page=20':[{'user':{'login':'Bob'},'body':'Comment'}],
'repos/a/b/pulls/1/comments?per_page=20':[{'user':{'login':'Bob'},'body':'Review','path':'src/a.rs'}],
'repos/a/b/issues?state=open&per_page=30':[issue,dict(issue,pull_request={})],
'repos/a/b/pulls?state=open&per_page=30':[issue],
'repos/a/b/commits?per_page=30':[{'sha':'1234567890abcdef','commit':{'message':'Title\nBody','author':{'name':'Ada'}}}],
'repos/a/b/commits?per_page=30&sha=feature':[{'sha':'1234567890abcdef','commit':{'message':'Title\nBody','author':{'name':'Ada'}}}],
'repos/a/b/releases?per_page=10':[{'tag_name':'v1','name':'Release','published_at':'2026-01-01T10:00:00Z','body':'Notes'}],
'users/a/repos?per_page=2':[{'id':7,'name':'b','description':'Répo'}],
}
raw={'a/b/main/raw.txt':'raw body'}
async def handler(req):
    assert req.headers.get('authorization')=='Bearer fixture-secret'
    path=req.url.path.lstrip('/');key=path+('?' + str(req.url.query,'utf-8') if req.url.query else '')
    if req.url.host=='raw.githubusercontent.com':
        return httpx.Response(200,text=raw[path]) if path in raw else httpx.Response(404)
    assert req.url.host=='api.github.com'
    return httpx.Response(200,json=responses[key]) if key in responses else httpx.Response(404)
urls=[
'https://github.com/a/b','https://github.com/a/b/compare/main...x','https://github.com/a/b/releases/download/v1/file.zip','https://github.com/a/b/commit/abcdef',
'https://github.com/a/b/blob/feature%2Fx/src/a.rs','https://github.com/a/b/issues/+1',
'https://github.com/a/b/blob/main/raw.txt','https://github.com/a/b/blob/main/src/a.rs','https://raw.githubusercontent.com/a/b/main/raw.txt',
'https://github.com/a/b/tree','https://github.com/a/b/tree/main/src','https://github.com/a/b/issues/1','https://github.com/a/b/pull/1','https://github.com/a/b/issues','https://github.com/a/b/pulls','https://github.com/a/b/commits','https://github.com/a/b/commits/feature','https://github.com/a/b/releases','https://api.github.com/users/a/repos?per_page=2',
'https://github.com/a','https://github.com/orgs/a','https://www.github.com/a/b','https://raw.githubusercontent.com/a/b',
]
# /orgs/a is parsed as owner='orgs', repo='a', per preserved source. It is
# NOT a special user/org extractor. Route exists but API failure falls through.
async def main():
    fetch._github_client=httpx.AsyncClient(transport=httpx.MockTransport(handler))
    chrome="---\nurl: private\n---\n# Heading\n\n![hero](https://example.com/h)\n\n[![Ada](https://example.com/a)](/author/ada/)\nBy [Ada](/author/ada/)\nPublished yesterday\nOriginally from elsewhere\nBio sentence\n\nArticle content.\n\n---\nPowered by [curl.md](https://curl.md)\n"
    result={'chrome':{'input':chrome,'expected':fetch._strip_provider_chrome(chrome)},'github_responses':responses,'raw_responses':raw,'github':[],'validation':[]}
    for url in urls:
        route=fetch._parse_github_url(url);record={'url':url,'route':route}
        if route:
            try:
                record['toon']=await fetch._github_fetch(route,'fixture-secret')
                original=fetch._to_toon;fetch._to_toon=lambda d:json.dumps(d,ensure_ascii=False,indent=2)
                try:record['fallback']=await fetch._github_fetch(route,'fixture-secret')
                finally:fetch._to_toon=original
            except httpx.HTTPStatusError:record['error']=True
        result['github'].append(record)
    cases=[
        ('InboxCategoryCreate',{'name':'Café 日本語'}),('InboxCategoryCreate',{'name':'A','slug':'x'*80}),
        ('InboxCategoryCreate',{'name':'A','slug':None}),('InboxCategoryCreate',{'name':'A','slug':''}),
        ('InboxCategoryCreate',{'name':'A','parent_id':'2.0','extra':'ignored'}),('InboxCategoryCreate',{'name':''}),
        ('InboxCategoryCreate',{'name':7}),('InboxCategoryCreate',{'name':'A','description':5}),
        ('InboxGhFolderCreate',{'name':'A'}),('InboxGhFolderCreate',{'name':'A','is_local':'false','is_pinned':True,'sort_order':9}),
        ('InboxGhFolderCreate',{'name':'A','is_local':0.0}),('InboxGhFolderCreate',{'name':'A','is_local':None}),
        ('InboxGhFolderPatch',{'name':None,'description':None,'is_pinned':None,'sort_order':None,'slug':'ignored'}),
        ('InboxGhFolderPatch',{'name':'A','sort_order':'7.0','is_pinned':'yes','icon':'ignored'}),
        ('InboxRuleCreate',{'name':'R','trigger':'star_synced','priority':'9999','enabled':'off','conditions':[{}]}),
        ('InboxRuleCreate',{'name':'R','trigger':'wrong'}),('InboxRuleCreate',{'name':'R','trigger':'star_synced','priority':10000}),
        ('InboxRuleUpdate',{'enabled':None}),('InboxRuleUpdate',{'priority':True,'conditions':[]} ),
        ('InboxRuleCreate',{'name':'R','trigger':'star_synced','actions':[1]}),
        ('InboxTeamCreate',{'name':'Team','slug':None}),
    ]
    for value in ['1_000','1e3',' 2 ','+2','00','-0.0','1.','1.000','nan','1.1',1.0,True]:
        cases.append(('InboxCategoryCreate',{'name':'Integer oracle','parent_id':value}))
    for name,body in cases:
        record={'schema':name,'body':body}
        try:
            m=getattr(schemas,name).model_validate(body)
            record['expected']=m.model_dump(exclude_unset=name=='InboxRuleUpdate',exclude_none=name=='InboxGhFolderPatch')
        except Exception:record['error']=True
        result['validation'].append(record)
    await fetch._github_client.aclose()
    print(json.dumps(result,ensure_ascii=False,indent=2))
asyncio.run(main())

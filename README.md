# iron-mockside

## Introduction
A mock web server useful for testing

The principle is similar to other mock servers: you can specify the response 
based on criteria in the request. 

What makes iron-mockside different is that it allows access to the full request
as one piece of text, both headers and body. It does not do any decoding or
processing.

Also it can be configured with timers: the same request will generate different 
responses depending on how long has passed since a certain moment. This way you
can simulate interactions like polling an endpoint for a result that changes in
time.

## Running iron-mockside

Current executable is built for linux x64 targets. Source code is available on
[github](https://github.com/ovidiu-ionescu/iron-mockside) if you want to 
compile it for a different target.

The command line allows to specify a ip address and port and a configuration 
file.

```json
 "run": "iron-mockside 0.0.0.0:8080 mocks/conf.txt
```
The command above will make _iron-mockside_ listen to all network adapters on 
port 8080, change directory to _mocks_ and read the _conf.txt_ file from there.


## Configuration

The configuration of iron-mockside is done via one config file. It contains a 
list of criteria and reponses separated by empty lines. 

Lines starting with _#_ (hash) are considered comments and ignored.

Each response is a group of lines. Last line in the group is a list 
of files separated by *;* (semicolon) that will be send back to the requester
concatenated in the order they are written in the line.
This allows to reuse some parts of the answer like subset of headers for example.

The reponse is literally what is contained in those files, including the http
status line.

All other lines are criteria to be searched in the request, body and
headers. First group from the config file to fully match will be the reply.

The default response is the 404.html file.

In the file line, if the first entry starts with \` (back tick) it means time.  
\`reset will set the internal timer to now
\`1000 means only execute this after 1000 milliseconds have passed 
since the timer was reset.  
Because first in the list is first served, put longer times first.

Example config file:

```
GET /hello
headers;hello.html

# reset internal timer
/reset
`reset;headers;reset.html

# only match 5000 milliseconds after reset
/time
`5000;headers;time2.html

# only match 1000 milliseconds after reset
/time
`1000;headers;time1.html

```

The location of the files that constitute the answer content is considered
relative to the location of the configuration file.

The Github repository contains a directory with a simple configuration example:
[mocks](https://github.com/ovidiu-ionescu/iron-mockside/tree/master/mocks)
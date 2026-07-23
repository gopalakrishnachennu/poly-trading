----------------------- MODULE MicroCapitalCanary -----------------------
EXTENDS Integers, FiniteSets, TLC
Scenarios=={"no_trade","eligible","floor","loss","exposure","allowlist","kill","deadman","abort","rollback"}
VARIABLES registered,approved,covered,kill,certified,capital,order,authority,halted
vars==<<registered,approved,covered,kill,certified,capital,order,authority,halted>>
Init==/\registered=FALSE/\approved=FALSE/\covered={}/\kill=FALSE/\certified=FALSE/\capital=FALSE/\order=FALSE/\authority=FALSE/\halted=FALSE
Register==/\~registered/\~halted/\registered'=TRUE/\UNCHANGED<<approved,covered,kill,certified,capital,order,authority,halted>>
Approve==/\registered/\~approved/\~halted/\approved'=TRUE/\UNCHANGED<<registered,covered,kill,certified,capital,order,authority,halted>>
Case(s)==/\s\in Scenarios/\approved/\~certified/\~halted/\(s="eligible"=>~kill)/\covered'=covered\cup{s}/\kill'=(IF s="kill" THEN TRUE ELSE kill)/\UNCHANGED<<registered,approved,certified,capital,order,authority,halted>>
Finalize==/\approved/\kill/\Scenarios\subseteq covered/\~certified/\~halted/\certified'=TRUE/\UNCHANGED<<registered,approved,covered,kill,capital,order,authority,halted>>
Fail==/\~certified/\~halted/\halted'=TRUE/\UNCHANGED<<registered,approved,covered,kill,certified,capital,order,authority>>
TerminalStutter==(certified\/halted)/\UNCHANGED vars
Next==Register\/Approve\/(\E s\in Scenarios:Case(s))\/Finalize\/Fail\/TerminalStutter
TypeInvariant==/\registered\in BOOLEAN/\approved\in BOOLEAN/\covered\subseteq Scenarios/\kill\in BOOLEAN/\certified\in BOOLEAN/\capital\in BOOLEAN/\order\in BOOLEAN/\authority\in BOOLEAN/\halted\in BOOLEAN
CompleteCertificate==certified=>kill/\Scenarios\subseteq covered
KillLatched==kill=>"kill"\in covered
NoCapital==~capital
NoOrder==~order
NoAuthority==~authority
HaltNotCertified==halted=>~certified
Spec==Init/\[][Next]_vars
=============================================================================

from html import escape
T="#7b8494"; B="#8792a2"; TEAL="#0d9488"; OR="#ea8a2e"; RED="#dc4a4a"
F="font-family='Inter, system-ui, sans-serif'"
p=[]
def rect(x,y,w,h,stroke=B,fill="none",op=1,rx=8,dash=None):
    d=f" stroke-dasharray='{dash}'" if dash else ""
    p.append(f"<rect x='{x}' y='{y}' width='{w}' height='{h}' rx='{rx}' fill='{fill}' fill-opacity='{op}' stroke='{stroke}' stroke-opacity='0.6'{d}/>")
def text(x,y,s,size=12,col=T,w=400,anchor="start"):
    p.append(f"<text x='{x}' y='{y}' font-size='{size}' fill='{col}' font-weight='{w}' text-anchor='{anchor}' {F}>{escape(s)}</text>")
def pill(x,y,s,col):
    w=8+6.6*len(s); p.append(f"<rect x='{x}' y='{y}' width='{w}' height='20' rx='10' fill='{col}' fill-opacity='0.15'/>"); text(x+w/2,y+14,s,11,col,600,"middle"); return x+w+8
def num(x,y,n):
    p.append(f"<circle cx='{x}' cy='{y}' r='11' fill='{OR}'/>"); text(x,y+4,str(n),12,"#ffffff",700,"middle")
W=760
rect(20,20,470,470)
x=pill(40,40,"Registered result",T); x=pill(x,40,"Strong evidence",TEAL); x=pill(x,40,"Compliance 100%",TEAL); pill(40,66,"Pre-trend: no divergence",T)
text(40,118,"Gross effect",12); text(40,156,"+12.30",36,TEAL,700); text(190,156,"per unit-day  (95% range 1.8 to 23.3)",12)
text(40,182,"gross +12.30 per unit-day on order_value; substitution not",11)
text(40,198,"excluded above … on full_price_value",11)
for i,(l,v) in enumerate([("Confidence","Passes the 95% bar"),("Treated units","16"),("Measured window","28 days"),("Total over treated","≈ $5,500")]):
    xx=40+i*112; rect(xx,216,104,58,rx=6); text(xx+8,236,l,10); text(xx+8,258,v,11,TEAL,600)
rect(40,288,430,64,rx=6,stroke=OR,dash="4 3"); text(52,308,"Projection to untreated units   [Assumption]",11,T,600); text(52,328,"If the treated units' effect held at the 24 others…",11)
rect(40,364,430,40,rx=6); text(52,389,"Day by day, across the whole run  ▸ chart and table",11)
rect(40,414,430,30,rx=6); text(52,434,"Predicted against measured · Proposed slope",11)
rect(40,452,430,30,rx=6); text(52,472,"Details · 8 checks · All clear",11)
notes=[(40,50,"Badges: is it a registered result, how strong is the evidence, did the change land, any warnings."),
 (40,140,"The headline: average change per unit per day, and the range the data cannot rule out."),
 (40,245,"The totals: exact arithmetic over the treated units, and the confidence bar."),
 (40,320,"The projection to units that were not treated. Always labelled an assumption."),
 (40,384,"The chart. The two groups should move together before the switch."),
 (40,429,"What it teaches the World Model, and the slope it can propose."),
 (40,467,"The checks: cannibalisation, promised size, compliance, pre-trend, randomisation.")]
for i,(x0,y,s) in enumerate(notes):
    num(505,y,i+1)
    words=s.split(); line=""; yy=y-3
    for w_ in words:
        if len(line)+len(w_)>34: text(524,yy,line,11); yy+=14; line=""
        line+=w_+" "
    text(524,yy,line,11)
p.insert(0,f"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {W} 510' width='100%' role='img'><title>How to read the report's summary card</title>")
p.append("</svg>")
open(__import__("pathlib").Path(__file__).resolve().parents[2] / "docs/images/experiments/report-anatomy.svg","w").write("".join(p))

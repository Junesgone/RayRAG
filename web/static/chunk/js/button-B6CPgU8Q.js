var N=Object.defineProperty,L=Object.defineProperties;var R=Object.getOwnPropertyDescriptors;var i=Object.getOwnPropertySymbols;var g=Object.prototype.hasOwnProperty,f=Object.prototype.propertyIsEnumerable;var m=(t,e,r)=>e in t?N(t,e,{enumerable:!0,configurable:!0,writable:!0,value:r}):t[e]=r,d=(t,e)=>{for(var r in e||(e={}))g.call(e,r)&&m(t,r,e[r]);if(i)for(var r of i(e))f.call(e,r)&&m(t,r,e[r]);return t},c=(t,e)=>L(t,R(e));var u=(t,e)=>{var r={};for(var o in t)g.call(t,o)&&e.indexOf(o)<0&&(r[o]=t[o]);if(t!=null&&i)for(var o of i(t))e.indexOf(o)<0&&f.call(t,o)&&(r[o]=t[o]);return r};import{r as h,j as s}from"./react-q9OOMZ1K.js";import{S as _}from"./@radix-ui-TBBTTv6g.js";import{c as C}from"./class-variance-authority-CRyEE5ya.js";import{c as l}from"../../entry/js/index-UFOmJYQ9.js";import{L as E}from"./react-router-y3MFLOVW.js";import{N as P,P as S}from"./lucide-react-EX5OqBLh.js";const V=C(l("inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm transition-colors outline-0","disabled:pointer-events-none disabled:opacity-50 rounded border-0.5 border-transparent",'[&_svg]:pointer-events-none [&_svg:not([class*="size-"])]:size-4 shrink-0 [&_svg]:shrink-0'),{variants:{variant:{default:"bg-text-primary text-bg-base shadow-xs hover:bg-text-primary/90 focus-visible:bg-text-primary/90",secondary:`
          bg-bg-card
          hover:text-text-primary hover:bg-border-button
          focus-visible:text-text-primary focus-visible:bg-border-button
        `,highlighted:`
          bg-text-primary text-bg-base border-b-4 border-b-accent-primary
          hover:bg-text-primary/90 focus-visible:bg-text-primary/90
        `,accent:`
          bg-accent-primary text-white
          hover:bg-accent-primary/90 focus-visible:bg-accent-primary/90
        `,destructive:`
          bg-state-error text-white shadow-xs
          hover:bg-state-error/90 focus-visible:ring-state-error/20 dark:focus-visible:ring-state-error/40
        `,outline:`
          text-text-secondary bg-bg-input border-0.5 border-border-button
          hover:text-text-primary hover:bg-border-button hover:border-border-default
          focus-visible:text-text-primary focus-visible:bg-border-button focus-visible:border-border-button
        `,dashed:`
          text-text-secondary border-border-button border-dashed
          hover:text-text-primary hover:bg-border-button hover:border-border-default
          focus-visible:text-text-primary focus-visible:bg-border-button focus-visible:border-border-button
        `,icon:"bg-transparent text-foreground hover:bg-transparent/80",transparent:`
          text-text-secondary bg-transparent border-0.5 border-border-button
          hover:text-text-primary hover:bg-border-button
          focus-visible:text-text-primary focus-visible:bg-border-button focus-visible:border-border-button
        `,danger:`
          bg-transparent border border-state-error text-state-error
          hover:bg-state-error/10 focus-visible:bg-state-error/10
        `,"danger-hover":`
          bg-bg-input border-border-button
          hover:bg-state-error/10 focus-visible:bg-state-error/10
          hover:text-state-error focus-visible:text-state-error
          hover:border-state-error focus-visible:border-state-error
        `,ghost:`
          text-text-secondary
          hover:bg-border-button focus-visible:bg-border-button
          hover:text-text-primary focus-visible:text-text-primary
        `,delete:`
          text-text-secondary
          hover:bg-state-error-5 hover:text-state-error
          focus-visible:text-state-error focus-visible:bg-state-error-5
        `,link:"text-primary underline-offset-4 hover:underline",static:"text-text-secondary hover:text-text-primary focus-visible:text-text-primary"},size:{auto:"",xl:"h-12 rounded-xl px-5 gap-3",lg:"h-10 rounded-lg px-4",default:"h-8 rounded px-3",sm:"h-7 rounded-sm px-2 gap-1",xs:"h-6 rounded-xs px-1 gap-0.5","icon-xl":"size-12 rounded-xl","icon-lg":"size-10 rounded-lg",icon:"size-8 rounded","icon-sm":"size-7 rounded-sm","icon-xs":"size-6 rounded-xs"}},defaultVariants:{variant:"default",size:"default"}}),p=h.forwardRef(function(q,j){var v=q,{children:e,className:r,variant:o,size:n,dot:b=!1,asChild:a=!1,asLink:y=!1,loading:x=!1,disabled:w=!1,block:z=!1}=v,B=u(v,["children","className","variant","size","dot","asChild","asLink","loading","disabled","block"]);const k=a?_:y?E:"button";return s.jsx(k,c(d({"data-inspector-line":"159","data-inspector-column":"4","data-inspector-relative-path":"src/components/ui/button.tsx",className:l(V({variant:o,size:n,className:r}),{"w-full":z},{relative:b}),ref:j,disabled:x||w},B),{children:s.jsxs(s.Fragment,{children:[b&&s.jsx("span",{"data-inspector-line":"172","data-inspector-column":"10","data-inspector-relative-path":"src/components/ui/button.tsx",className:"absolute size-[6px] rounded-full -right-[3px] -top-[3px] bg-state-error animate"}),x&&s.jsx(P,{"data-inspector-line":"174","data-inspector-column":"20","data-inspector-relative-path":"src/components/ui/button.tsx",className:"animate-spin"}),e]})}))});p.displayName="Button";const F=p;F.displayName="ButtonLoading";const M=h.forwardRef(function(b,n){var a=b,{children:e,className:r}=a,o=u(a,["children","className"]);return s.jsxs(p,c(d({"data-inspector-line":"192","data-inspector-column":"6","data-inspector-relative-path":"src/components/ui/button.tsx",variant:"outline",ref:n,className:l("w-full border-dashed border-input-border",r)},o),{children:[s.jsx(S,{"data-inspector-line":"198","data-inspector-column":"8","data-inspector-relative-path":"src/components/ui/button.tsx"})," ",e]}))});export{F as B,p as a,V as b,M as c};

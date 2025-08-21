<!doctype html>
<html lang=en>
    <head>
        <meta charset=utf-8>
        <meta content="width=device-width,initial-scale=1" name=viewport>
        <title>Blog</title>
        <meta content="This is my blog where I share my experiences, documenting the things I have built and the valuable lessons I have learned along the way." name=description>
        <style>
            * {
                font-family: system-ui,-apple-system,BlinkMacSystemFont,Segoe UI, Roboto,Oxygen,Ubuntu,Cantarell,Open Sans,Helvetica Neue, sans-serif
            }

            #container {
                display: flex;
                flex-direction: column;
                margin: 0 auto;
                max-width: 953px;
                margin-bottom: 60px;
                font-size: 18px;
                line-height: 35px
            }

            #container h1,#container h2,#container h3,#container h4,#container h5,#container h6 {
                margin: 0;
                padding: 0;
                margin-top: 15px;
                margin-bottom: 10px
            }

            #container ol,#container ul {
                margin: 0;
                padding-inline-start:21px}

            #container p {
                margin: 0;
                padding: 0;
                margin-top: 5px;
                margin-bottom: 5px
            }

            #container p img {
                display: block;
                margin-left: auto;
                margin-right: auto;
                width: auto;
                max-width: 100%
            }

            #container pre {
                padding: 11px;
                border-radius: .25rem;
                margin-top: 0
            }

            #container center i {
                color: gray
            }

            #container a {
                font-family: sans-serif;
                margin: 0;
                width: fit-content;
                padding: 0;
                -webkit-font-smoothing: antialiased;
                text-rendering: optimizespeed;
                color: #2583fd;
                border-radius: 3px;
                transition: all .1s
            }

            #container a:hover {
                background: #ecf3fc
            }

            body {
                margin: 0;
                padding: 0;
                min-height: 100vh;
                background: white url(https://i.vldr.org/tN6Yfum.svg) no-repeat;
                background-size: 100vw
            }

            .image {
                width: 100%;
                max-height: 480px;
                max-width: 960px;
                border-radius: .5rem;
                object-fit: cover;
                margin-top: 39px;
                margin-bottom: 24px
            }

            .title {
                font: 400 2.5em/65px sans-serif;
                line-height: 65px;
                -webkit-font-smoothing: antialiased;
                text-rendering: optimizespeed;
                margin-bottom: 0!important
            }

            .subtitle {
                text-rendering: optimizespeed;
                font: 25px/38px sans-serif;
                margin-bottom: 10px!important;
                -webkit-font-smoothing: antialiased
            }

            .subtitle span {
                text-rendering: optimizespeed;
                font: 17px/28px sans-serif;
                color: gray;
                margin: 0;
                padding: 0;
                margin-bottom: 20px;
                -webkit-font-smoothing: antialiased
            }

            #navbar {
                height: 48px;
                background: white;
                box-shadow: 0 2px 2px 0 #0000000f
            }

            #navbar-container {
                margin: 0 auto;
                max-width: 953px;
                height: 100%;
                align-items: center;
                display: flex
            }

            #navbar-logo {
                height: 100%;
                display: flex;
                align-items: end;
                transition: all .1s
            }

            #navbar-logo:hover {
                opacity: .5
            }

            #navbar-links {
                flex: 1;
                display: flex;
                justify-content: end;
                gap: 1.2em
            }

            #navbar-links a {
                color: gray;
                text-decoration: none;
                font-weight: 400;
                font-size: 1rem;
                line-height: 1.75;
                text-rendering: optimizespeed
            }

            #navbar-links a:hover {
                color: black
            }

            #title {
                -webkit-font-smoothing: antialiased!important;
                font: 400 1.75em sans-serif!important;
                text-rendering: optimizespeed!important;
                margin: 0!important;
                padding: 0!important;
                margin-top: 39px!important;
                margin-bottom: 10px!important
            }

            #description {
                -webkit-font-smoothing: antialiased!important;
                font: 19px/28px sans-serif!important;
                color: gray!important;
                margin: 0!important;
                padding: 0!important;
                text-rendering: optimizespeed!important;
                margin-bottom: 10px!important
            }

            #grid {
                display: flex;
                flex-direction: column;
                gap: 2rem;
                margin-top: 20px
            }

            .grid-item {
                display: flex;
                flex-direction: row;
                overflow: hidden;
                height: fit-content;
                width: auto;
                background: white;
                border: 1px solid #dadce0;
                box-shadow: rgba(0,0,0,.06) 0 5px 5px 0;
                border-radius: .75rem
            }

            .grid-item img {
                height: 260px;
                width: 260px;
                object-fit: cover
            }

            .grid-subitem {
                display: flex;
                flex-direction: column;
                padding-left: 25px;
                padding-right: 25px
            }

            .grid-subitem p {
                font-size: 20px!important;
                flex: 1;
                margin-top: 5px!important;
                line-height: 30px!important
            }

            .grid-subitem a {
                -webkit-font-smoothing: antialiased!important;
                font: 400 1.9em sans-serif!important;
                text-rendering: optimizespeed!important;
                color: black!important;
                text-decoration: none;
                margin: 0!important;
                padding: 0!important;
                margin-top: 22px!important
            }

            .grid-content {
                font-size: 16px;
                line-height: 22px;
                margin-bottom: 28px
            }

            .grid-keywords {
                display: flex;
                flex-direction: row;
                gap: .3rem;
                margin-bottom: 15px
            }

            .grid-keywords div {
                border-radius: 16px;
                user-select: none;
                font-size: 14px;
                line-height: 19px;
                border: 1px solid lightgrey;
                padding: 5px 15px
            }

            p code {
                background: #e5e5e5;
                padding: 4px 8px;
                border-radius: 6px
            }

            @media screen and (max-width: 1042px) {
                .grid-item {
                    flex-direction:column
                }

                .grid-subitem a {
                    font-size: 2em!important
                }

                .grid-subitem p {
                    font-size: 16px!important;
                    line-height: 26px!important
                }

                .grid-item img {
                    object-fit: cover;
                    height: unset;
                    width: unset;
                    max-height: 15rem
                }

                .grid-keywords {
                    margin-top: 16px
                }

                .grid-keywords div {
                    font-size: 12px
                }

                #container,#navbar-container {
                    padding-left: 30px;
                    padding-right: 30px
                }
            }
        </style>
    <body>
        <div id=navbar>
            <div id=navbar-container>
                <a href=https://vldr.org/ id=navbar-logo>
                    <svg viewbox="0 0 105 44" height=42 version=1.2 width=100 xmlns=http://www.w3.org/2000/svg>
                        <style>
                            .a {
                                fill: #fff
                            }

                            .b {
                                fill: #09f
                            }

                            .c {
                                fill: #7cde00
                            }

                            .d {
                                fill: #fefe00
                            }

                            .e {
                                fill: #ff8000
                            }

                            .f {
                                fill: #f00
                            }
                        </style>
                        <path d="m25.1 19.3v-3.7h-7.9v3.7h1.3l-2.5 7.3h-0.3l-3.4-7.3h1.7v-3.7h-8.9v3.7h1.9l6.5 13.7h3.6l6.2-13.7zm10 13.7v-3.7h-2.3v-22.8h-7.5v3.7h2.5v19.1h-2.3v3.7zm22.1 0v-3.7h-2.3v-22.8h-8.4v3.7h3.4v8.1c-0.9-1.8-2.7-3-5.5-3-5.2 0-7.8 4.2-7.8 9.3 0 5.4 2.8 8.8 7.4 8.8 3.1 0 4.9-1.6 5.9-3.6v3.2zm-7.3-8.8c0 2.6-1.6 5.3-4.2 5.3-2.1 0-3.9-1.9-3.9-5.2 0-3.5 2-5.1 4.2-5.1 2.2 0 3.9 1.7 3.9 4.6zm20-8.8c-2.4 0-3.5 1.5-4.1 3.5v-3.3h-7.1v3.8h2.5v9.8h-2.3v3.8h10.4v-3.8h-3.1v-7.2c0-1.7 1-2.6 2.1-2.6 0.5 0 0.8 0.2 1.3 0.5l0.7 2.9 3.8-0.4-0.8-6.1c-1.2-0.7-2.5-0.9-3.4-0.9zm7.5 4.1h3.2v-3.6h-3.2zm0 13.5h3.2v-3.7h-3.2zm17.5-26.5h-2.1l-8.5 26.5h2.1zm6.2 0h-2.1l-8.5 26.6h2.1z"></path>
                        <path d="m0 41h21v3h-21z" class=b></path>
                        <path d="m21 41h21v3h-21z" class=c></path>
                        <path d="m42 41h21v3h-21z" class=d></path>
                        <path d="m63 41h21v3h-21z" class=e></path>
                        <path d="m84 41h21v3h-21z" class=f></path>
                    </svg>
                </a>
                <div id=navbar-links>
                    <a href=/blog>Blog</a>
                    <a href=/contact>Contact</a>
                </div>
            </div>
        </div>
        <div id=container>
            <p id=title>Blog
            <p id=description>This is my blog where I share my experiences, documenting the things I have built and the valuable lessons I have learned along the way.
            <div id=grid>
                <?
                    Database connection = Database("examples/blog.db")
                    Statement stmt = connection.prepare("SELECT * FROM posts WHERE is_hidden = 0 ORDER BY date DESC")

                    while stmt and stmt.next()
                        string keywords
                        for string keyword in stringSplit(stmt.read<string>("keywords"), ',')
                            if keyword
                                keywords += "<div>" + keyword + "</div>"

                        print("<div class=grid-item>
                            <img src=" + stmt.read<string>("image") + ">
                            <div class=grid-subitem>
                                <a href=view.cy?id=" + stmt.read<int>("id") + ">" + stmt.read<string>("title") + "</a>
                                <p>" + stmt.read<string>("desc") + "</p>
                                <div class=grid-keywords>
                                    " + keywords + "
                                </div>
                            </div>
                        </div>")
                ?>
            </div>
        </div>
    </body>
</html>
use std::borrow::Cow;

use glium::{
    glutin::{self, Event, WindowEvent},
    VertexBuffer,
    texture::{
        ClientFormat, RawImage2d,
    },
    Program,
    Display, Surface, Texture2d, index::PrimitiveType,
};

pub fn render<F: FnMut(&Display) -> Option<Texture2d>>(mut f: F) {
    let mut events_loop = glutin::EventsLoop::new();
    let context = glutin::ContextBuilder::new().with_vsync(true);
    let builder = glutin::WindowBuilder::new()
        .with_title("Video Ludo example".to_owned())
        .with_dimensions(glutin::dpi::LogicalSize::new(1024f64, 768f64));

    let display =
        Display::new(builder, context, &events_loop).expect("Failed to initialize display");

    let raw = RawImage2d {
        data: Cow::Owned(vec![0u8; 4 * 3]),
        width: 2,
        height: 2,
        format: ClientFormat::U8U8U8,
    };

    let def_texture = Texture2d::new(&display, raw).unwrap();
    let mut texture: Option<Texture2d> = None;

    let index_buffer = glium::IndexBuffer::new(&display, PrimitiveType::TriangleStrip, &[1u16, 2, 0, 3]).unwrap();

    // building the vertex buffer, which contains all the vertices that we will draw
    let vertex_buffer = {
        #[derive(Copy, Clone)]
        struct Vertex {
            position: [f32; 2],
            tex_coords: [f32; 2],
        }

        implement_vertex!(Vertex, position, tex_coords);

        VertexBuffer::new(&display, 
            &[
                Vertex { position: [-1.0, -1.0], tex_coords: [0.0, 0.0] },
                Vertex { position: [-1.0,  1.0], tex_coords: [0.0, 1.0] },
                Vertex { position: [ 1.0,  1.0], tex_coords: [1.0, 1.0] },
                Vertex { position: [ 1.0, -1.0], tex_coords: [1.0, 0.0] }
            ]
        ).unwrap()
    };

    let program = create_program(&display);


    let mut run = true;
    while run {
        events_loop.poll_events(|event| {
            if let Event::WindowEvent { event, .. } = event {
                if let WindowEvent::CloseRequested = event {
                    run = false;
                }
            }
        });

        // Take the new texture or keep the old one
        texture = f(&display).or(texture);

        let uniforms = uniform!{
            matrix: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, -1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0f32]
            ],
            tex: texture.as_ref().unwrap_or(&def_texture),
        };

        let mut target = display.draw();
        target.clear_color_srgb(1.0, 1.0, 1.0, 1.0);

        target.draw(&vertex_buffer, &index_buffer, &program, &uniforms, &Default::default()).unwrap();

        target.finish().expect("Failed to swap buffers");
    }
}

fn create_program(display: &Display) -> Program {
    program!(display,
        140 => {
            vertex: "
                #version 140
                uniform mat4 matrix;
                in vec2 position;
                in vec2 tex_coords;
                out vec2 v_tex_coords;
                void main() {
                    gl_Position = matrix * vec4(position, 0.0, 1.0);
                    v_tex_coords = tex_coords;
                }
            ",

            fragment: "
                #version 140
                uniform sampler2D tex;
                in vec2 v_tex_coords;
                out vec4 f_color;
                void main() {
                    f_color = texture(tex, v_tex_coords);
                }
            "
        },

        110 => {  
            vertex: "
                #version 110
                uniform mat4 matrix;
                attribute vec2 position;
                attribute vec2 tex_coords;
                varying vec2 v_tex_coords;
                void main() {
                    gl_Position = matrix * vec4(position, 0.0, 1.0);
                    v_tex_coords = tex_coords;
                }
            ",

            fragment: "
                #version 110
                uniform sampler2D tex;
                varying vec2 v_tex_coords;
                void main() {
                    gl_FragColor = texture2D(tex, v_tex_coords);
                }
            ",
        },

        100 => {  
            vertex: "
                #version 100
                uniform lowp mat4 matrix;
                attribute lowp vec2 position;
                attribute lowp vec2 tex_coords;
                varying lowp vec2 v_tex_coords;
                void main() {
                    gl_Position = matrix * vec4(position, 0.0, 1.0);
                    v_tex_coords = tex_coords;
                }
            ",

            fragment: "
                #version 100
                uniform lowp sampler2D tex;
                varying lowp vec2 v_tex_coords;
                void main() {
                    gl_FragColor = texture2D(tex, v_tex_coords);
                }
            ",
        },
    ).unwrap()
}
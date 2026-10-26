// GPUI iOS Example - Main Entry Point
//
// This is a minimal iOS app that demonstrates GPUI running on iOS.
// When USE_GPUI_RUST is defined, it links against the GPUI Rust static library
// and uses GPUI for all rendering.

#import <UIKit/UIKit.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>

// Define USE_GPUI_RUST to enable Rust GPUI integration
// This requires linking against libgpui.a
#ifdef USE_GPUI_RUST
#import "gpui_ios.h"
// Exported by the leaf-ios Rust static lib: run a formatting command on the
// editor. Ids match apps/leaf-ios/src/lib.rs (leaf_ios_cmd).
extern void leaf_ios_cmd(uint32_t cmd_id);
#endif

#ifndef USE_GPUI_RUST
// Fallback Metal view for when GPUI is not linked
@interface GPUIMetalView : UIView
@property (nonatomic, strong) id<MTLDevice> device;
@property (nonatomic, strong) id<MTLCommandQueue> commandQueue;
@end

@implementation GPUIMetalView

+ (Class)layerClass {
    return [CAMetalLayer class];
}

- (instancetype)initWithFrame:(CGRect)frame {
    self = [super initWithFrame:frame];
    if (self) {
        [self setupMetal];
    }
    return self;
}

- (void)setupMetal {
    self.device = MTLCreateSystemDefaultDevice();
    if (!self.device) {
        NSLog(@"Metal is not supported on this device");
        return;
    }

    CAMetalLayer *metalLayer = (CAMetalLayer *)self.layer;
    metalLayer.device = self.device;
    metalLayer.pixelFormat = MTLPixelFormatBGRA8Unorm;
    metalLayer.framebufferOnly = YES;
    metalLayer.contentsScale = [UIScreen mainScreen].scale;

    self.commandQueue = [self.device newCommandQueue];

    NSLog(@"Metal initialized successfully with device: %@", self.device.name);
}

- (void)drawRect:(CGRect)rect {
    CAMetalLayer *metalLayer = (CAMetalLayer *)self.layer;
    id<CAMetalDrawable> drawable = [metalLayer nextDrawable];
    if (!drawable) return;

    MTLRenderPassDescriptor *passDescriptor = [MTLRenderPassDescriptor renderPassDescriptor];
    passDescriptor.colorAttachments[0].texture = drawable.texture;
    passDescriptor.colorAttachments[0].loadAction = MTLLoadActionClear;
    passDescriptor.colorAttachments[0].storeAction = MTLStoreActionStore;
    // Catppuccin Mocha base color
    passDescriptor.colorAttachments[0].clearColor = MTLClearColorMake(0.118, 0.118, 0.180, 1.0);

    id<MTLCommandBuffer> commandBuffer = [self.commandQueue commandBuffer];
    id<MTLRenderCommandEncoder> encoder = [commandBuffer renderCommandEncoderWithDescriptor:passDescriptor];
    [encoder endEncoding];

    [commandBuffer presentDrawable:drawable];
    [commandBuffer commit];
}

@end

// Fallback View Controller for non-GPUI mode
@interface GPUIFallbackViewController : UIViewController
@property (nonatomic, strong) GPUIMetalView *metalView;
@property (nonatomic, strong) UILabel *statusLabel;
@property (nonatomic, strong) CADisplayLink *displayLink;
@end

@implementation GPUIFallbackViewController

- (void)viewDidLoad {
    [super viewDidLoad];

    self.metalView = [[GPUIMetalView alloc] initWithFrame:self.view.bounds];
    self.metalView.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    [self.view addSubview:self.metalView];

    self.statusLabel = [[UILabel alloc] init];
    self.statusLabel.text = @"GPUI iOS\nMetal Fallback Mode";
    self.statusLabel.numberOfLines = 0;
    self.statusLabel.textAlignment = NSTextAlignmentCenter;
    self.statusLabel.textColor = [UIColor colorWithRed:0.804 green:0.839 blue:0.957 alpha:1.0];
    self.statusLabel.font = [UIFont systemFontOfSize:24 weight:UIFontWeightBold];
    self.statusLabel.translatesAutoresizingMaskIntoConstraints = NO;
    [self.view addSubview:self.statusLabel];

    [NSLayoutConstraint activateConstraints:@[
        [self.statusLabel.centerXAnchor constraintEqualToAnchor:self.view.centerXAnchor],
        [self.statusLabel.centerYAnchor constraintEqualToAnchor:self.view.centerYAnchor],
    ]];

    self.displayLink = [CADisplayLink displayLinkWithTarget:self selector:@selector(render)];
    [self.displayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];

    NSLog(@"GPUI iOS Fallback Mode Started");
}

- (void)render {
    [self.metalView setNeedsDisplay];
}

- (void)viewDidLayoutSubviews {
    [super viewDidLayoutSubviews];
    CAMetalLayer *metalLayer = (CAMetalLayer *)self.metalView.layer;
    CGFloat scale = [UIScreen mainScreen].scale;
    metalLayer.drawableSize = CGSizeMake(self.metalView.bounds.size.width * scale,
                                          self.metalView.bounds.size.height * scale);
}

- (UIStatusBarStyle)preferredStatusBarStyle {
    return UIStatusBarStyleLightContent;
}

- (void)dealloc {
    [self.displayLink invalidate];
}

@end
#endif // !USE_GPUI_RUST

#ifdef USE_GPUI_RUST
// ── Formatting toolbar (keyboard accessory view) ─────────────────────────────
// A native UIToolbar docked above the software keyboard. Each button calls
// leaf_ios_cmd(id), which re-enters gpui and runs the editor command. This is
// the native-shell counterpart to the desktop's ⌘-key formatting actions.

@interface LeafToolbarActions : NSObject
@end
@implementation LeafToolbarActions
- (void)bold        { leaf_ios_cmd(0); }
- (void)italic      { leaf_ios_cmd(1); }
- (void)code        { leaf_ios_cmd(2); }
- (void)h1          { leaf_ios_cmd(3); }
- (void)h2          { leaf_ios_cmd(4); }
- (void)body        { leaf_ios_cmd(5); }
- (void)toggleView  { leaf_ios_cmd(6); }
- (void)undo        { leaf_ios_cmd(7); }
- (void)redo        { leaf_ios_cmd(8); }
@end

static LeafToolbarActions *gLeafToolbarActions = nil;
static UIToolbar *gLeafToolbar = nil;

// An SF Symbol button, falling back to a text title if the symbol is
// unavailable (so a button is never blank).
static UIBarButtonItem *LeafSym(NSString *symbol, NSString *fallback, SEL action) {
    UIImage *img = [UIImage systemImageNamed:symbol];
    if (img) {
        return [[UIBarButtonItem alloc] initWithImage:img
                                                style:UIBarButtonItemStylePlain
                                               target:gLeafToolbarActions
                                               action:action];
    }
    return [[UIBarButtonItem alloc] initWithTitle:fallback
                                            style:UIBarButtonItemStylePlain
                                           target:gLeafToolbarActions
                                           action:action];
}

static UIBarButtonItem *LeafText(NSString *title, SEL action) {
    return [[UIBarButtonItem alloc] initWithTitle:title
                                            style:UIBarButtonItemStylePlain
                                           target:gLeafToolbarActions
                                           action:action];
}

static UIBarButtonItem *LeafFlex(void) {
    return [[UIBarButtonItem alloc]
        initWithBarButtonSystemItem:UIBarButtonSystemItemFlexibleSpace target:nil action:nil];
}

static void leaf_install_toolbar(void) {
    gLeafToolbarActions = [[LeafToolbarActions alloc] init];
    CGFloat width = [UIScreen mainScreen].bounds.size.width;
    UIToolbar *tb = [[UIToolbar alloc] initWithFrame:CGRectMake(0, 0, width, 44)];

    // SF Symbols keep the buttons compact and native; headings stay as short
    // text since there is no clean symbol for them.
    NSArray<UIBarButtonItem *> *buttons = @[
        LeafSym(@"bold",   @"B",   @selector(bold)),
        LeafSym(@"italic", @"I",   @selector(italic)),
        LeafSym(@"chevron.left.forwardslash.chevron.right", @"</>", @selector(code)),
        LeafText(@"H1",  @selector(h1)),
        LeafText(@"H2",  @selector(h2)),
        LeafSym(@"paragraphsign", @"¶", @selector(body)),
        LeafSym(@"textformat",    @"Aa", @selector(toggleView)),
        LeafSym(@"arrow.uturn.backward", @"Undo", @selector(undo)),
        LeafSym(@"arrow.uturn.forward",  @"Redo", @selector(redo)),
    ];

    // Interleave flexible spaces so the buttons spread evenly across the bar
    // instead of clustering on the left and overflowing.
    NSMutableArray<UIBarButtonItem *> *items = [NSMutableArray array];
    [items addObject:LeafFlex()];
    for (NSUInteger i = 0; i < buttons.count; i++) {
        [items addObject:buttons[i]];
        [items addObject:LeafFlex()];
    }
    tb.items = items;

    [tb sizeToFit];
    gLeafToolbar = tb;
    gpui_ios_set_input_accessory_view((__bridge void *)tb);
    NSLog(@"leaf-ios: formatting toolbar installed");
}
#endif // USE_GPUI_RUST

// App Delegate
@interface GPUIAppDelegate : UIResponder <UIApplicationDelegate>
@property (nonatomic, strong) UIWindow *window;
#ifdef USE_GPUI_RUST
@property (nonatomic, assign) void *gpuiApp;
@property (nonatomic, assign) void *gpuiWindow;
@property (nonatomic, strong) CADisplayLink *displayLink;
#endif
@end

@implementation GPUIAppDelegate

- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    NSLog(@"GPUI iOS Application Launching...");

#ifdef USE_GPUI_RUST
    // Register the example app's root view (Router), then start GPUI.
    // gpui_ios_register_app() is defined in the example crate and sets up
    // the callback that creates the Router with all screens + demos.
    // gpui_ios_run_demo() is defined in gpui-mobile and starts the run loop.
    NSLog(@"Starting GPUI app...");
    gpui_ios_register_app();
    gpui_ios_run_demo();
    NSLog(@"GPUI app initialized");

    // Get the GPUI window pointer that was created
    self.gpuiWindow = gpui_ios_get_window();
    if (self.gpuiWindow) {
        NSLog(@"Got GPUI window pointer: %p", self.gpuiWindow);

        // Setup CADisplayLink to drive rendering
        self.displayLink = [CADisplayLink displayLinkWithTarget:self selector:@selector(renderFrame)];
        [self.displayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
        NSLog(@"CADisplayLink started for GPUI rendering");

        // Install the native formatting toolbar as the keyboard accessory view.
        leaf_install_toolbar();
    } else {
        NSLog(@"Warning: No GPUI window was created");
    }

    NSLog(@"GPUI iOS Application Launched with Rust integration");
#else
    // Fallback mode: create our own window with demo UI
    self.window = [[UIWindow alloc] initWithFrame:[UIScreen mainScreen].bounds];
    self.window.rootViewController = [[GPUIFallbackViewController alloc] init];
    [self.window makeKeyAndVisible];
    NSLog(@"GPUI iOS Application Launched in fallback mode");
#endif

    return YES;
}

#ifdef USE_GPUI_RUST
- (void)renderFrame {
    if (self.gpuiWindow) {
        gpui_ios_request_frame(self.gpuiWindow);
    }
}
#endif

- (void)applicationWillEnterForeground:(UIApplication *)application {
    NSLog(@"GPUI iOS: Will enter foreground");
#ifdef USE_GPUI_RUST
    gpui_ios_will_enter_foreground(self.gpuiApp);

    // Resume display link when coming to foreground
    if (!self.displayLink && self.gpuiWindow) {
        self.displayLink = [CADisplayLink displayLinkWithTarget:self selector:@selector(renderFrame)];
        [self.displayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
    }
#endif
}

- (void)applicationDidBecomeActive:(UIApplication *)application {
    NSLog(@"GPUI iOS: Did become active");
#ifdef USE_GPUI_RUST
    gpui_ios_did_become_active(self.gpuiApp);
#endif
}

- (void)applicationWillResignActive:(UIApplication *)application {
    NSLog(@"GPUI iOS: Will resign active");
#ifdef USE_GPUI_RUST
    gpui_ios_will_resign_active(self.gpuiApp);
#endif
}

- (void)applicationDidEnterBackground:(UIApplication *)application {
    NSLog(@"GPUI iOS: Did enter background");
#ifdef USE_GPUI_RUST
    gpui_ios_did_enter_background(self.gpuiApp);

    // Pause display link when going to background to save power
    if (self.displayLink) {
        [self.displayLink invalidate];
        self.displayLink = nil;
    }
#endif
}

- (BOOL)application:(UIApplication *)application openURL:(NSURL *)url options:(NSDictionary<UIApplicationOpenURLOptionsKey, id> *)options {
    NSLog(@"GPUI iOS: Open URL: %@", url);
#ifdef USE_GPUI_RUST
    NSString *urlString = [url absoluteString];
    gpui_ios_handle_open_url((__bridge void *)urlString);
#endif
    return YES;
}

- (void)applicationWillTerminate:(UIApplication *)application {
    NSLog(@"GPUI iOS: Will terminate");
#ifdef USE_GPUI_RUST
    if (self.displayLink) {
        [self.displayLink invalidate];
        self.displayLink = nil;
    }
    gpui_ios_will_terminate(self.gpuiApp);
#endif
}

@end

// Main entry point
int main(int argc, char * argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil, NSStringFromClass([GPUIAppDelegate class]));
    }
}
